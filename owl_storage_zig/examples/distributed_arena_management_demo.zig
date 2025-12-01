// Distributed Arena Management Demo
//
// This example demonstrates the comprehensive distributed arena management
// functionality including shard coordination, cross-shard operations,
// garbage collection, and migration capabilities.

const std = @import("std");
const owl = @import("storage");

const GlobalObjRef = owl.GlobalObjRef;
const ShardId = owl.ShardId;
const Arena = owl.Arena;
const Object = owl.Object;
const ObjRef = owl.ObjRef;
const DistributedArenaManager = owl.DistributedArenaManager;
const ShardStatus = owl.ShardStatus;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("=== Distributed Arena Management Demo ===\n\n", .{});

    // Create distributed arena manager
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    // Create multiple arenas representing different shards
    var tmp_dirs = [_]std.testing.TmpDir{
        std.testing.tmpDir(.{}),
        std.testing.tmpDir(.{}),
        std.testing.tmpDir(.{}),
        std.testing.tmpDir(.{}),
    };
    defer for (&tmp_dirs) |*dir| dir.cleanup();

    var arenas: [4]*Arena = undefined;
    var arena_paths: [4][]u8 = undefined;

    // Initialize arenas for shards 1-4
    for (0..4) |i| {
        const shard_id: ShardId = @intCast(i + 1);
        const path = try tmp_dirs[i].dir.realpathAlloc(allocator, ".");
        defer allocator.free(path);

        const file_name = try std.fmt.allocPrint(allocator, "shard_{}.dat", .{shard_id});
        defer allocator.free(file_name);

        arena_paths[i] = try std.fs.path.join(allocator, &[_][]const u8{ path, file_name });
        arenas[i] = try allocator.create(Arena);
        arenas[i].* = try Arena.init(allocator, arena_paths[i], 4 * 1024 * 1024, shard_id); // 4MB per shard
    }

    defer {
        for (0..4) |i| {
            arenas[i].deinit();
            allocator.destroy(arenas[i]);
            allocator.free(arena_paths[i]);
        }
    }

    // Add all shards to the distributed manager
    for (0..4) |i| {
        const shard_id: ShardId = @intCast(i + 1);
        try manager.addShard(shard_id, arenas[i]);
    }

    std.debug.print("Created distributed arena manager with 4 shards:\n", .{});
    for (1..5) |shard_id| {
        const status = manager.getShardStatus(@intCast(shard_id));
        std.debug.print("  Shard {}: {}\n", .{ shard_id, status });
    }
    std.debug.print("\n", .{});

    // === Demonstrate distributed allocation ===
    std.debug.print("=== Distributed Allocation ===\n", .{});

    var object_refs: [12]GlobalObjRef = undefined;
    var objects: [12]Object = undefined;

    // Create objects across different shards
    for (0..12) |i| {
        const shard_id: ShardId = @intCast((i % 4) + 1);
        const object_id = [_]u8{@intCast(i)} ** 16;
        const content = try std.fmt.allocPrint(allocator, "Object {} in shard {}", .{ i, shard_id });
        defer allocator.free(content);

        objects[i] = try Object.init(allocator, @intCast(i + 1), object_id, content, &[_]u8{}, &[_]ObjRef{});

        const data = try objects[i].toBytes(allocator);
        defer allocator.free(data);

        object_refs[i] = try manager.allocateDistributed(shard_id, @intCast(data.len));
        const arena = arenas[shard_id - 1];
        try arena.writeGlobal(object_refs[i], data);

        std.debug.print("  Object {}: allocated in shard {} at offset {}\n", .{
            i,
            object_refs[i].getShardId(),
            object_refs[i].getLocalOffset(),
        });
    }
    std.debug.print("\n", .{});

    // === Demonstrate cross-shard navigation ===
    std.debug.print("=== Cross-Shard Navigation ===\n", .{});

    for (0..4) |i| {
        const ref = object_refs[i * 3]; // Pick every 3rd object
        const nav_result = manager.navigateDistributed(ref);

        if (nav_result.isLocal()) {
            var retrieved_obj = nav_result.getLocal().?;
            defer retrieved_obj.deinit();
            std.debug.print("  ✓ Local navigation to object in shard {}: \"{s}\"\n", .{
                ref.getShardId(),
                retrieved_obj.fixed,
            });
        } else if (nav_result.isRemote()) {
            const remote_info = nav_result.getRemoteInfo().?;
            std.debug.print("  → Remote reference detected: shard {}, offset {}\n", .{
                remote_info.shard_id,
                remote_info.objref.getLocalOffset(),
            });
        }
    }
    std.debug.print("\n", .{});

    // === Demonstrate cross-shard reference resolution ===
    std.debug.print("=== Cross-Shard Reference Resolution ===\n", .{});

    for (0..3) |i| {
        const ref = object_refs[i * 4]; // Pick every 4th object
        const resolved = try manager.resolveCrossShardRef(ref);

        if (resolved) |obj| {
            var mutable_obj = obj;
            defer mutable_obj.deinit();
            std.debug.print("  ✓ Resolved object from shard {}: \"{s}\"\n", .{
                ref.getShardId(),
                mutable_obj.fixed,
            });
        } else {
            std.debug.print("  ✗ Failed to resolve object from shard {}\n", .{ref.getShardId()});
        }
    }
    std.debug.print("\n", .{});

    // === Demonstrate co-location ===
    std.debug.print("=== Co-location Demonstration ===\n", .{});

    const base_ref = try manager.allocateDistributed(2, 1000);
    const colocated_ref = try manager.allocateNearDistributed(2, 1000, base_ref);

    std.debug.print("  Base object: shard {}, page {}, slot {}\n", .{
        base_ref.getShardId(),
        base_ref.pageId(),
        base_ref.slotId(),
    });
    std.debug.print("  Co-located object: shard {}, page {}, slot {}\n", .{
        colocated_ref.getShardId(),
        colocated_ref.pageId(),
        colocated_ref.slotId(),
    });

    if (base_ref.pageId() == colocated_ref.pageId()) {
        std.debug.print("  ✓ Objects successfully co-located on same page!\n", .{});
    } else {
        std.debug.print("  ○ Objects on different pages (still acceptable)\n", .{});
    }
    std.debug.print("\n", .{});

    // === Demonstrate shard status management ===
    std.debug.print("=== Shard Status Management ===\n", .{});

    // Set shard 3 to migrating status
    try manager.setShardStatus(3, .migrating);
    std.debug.print("  Set shard 3 to migrating status\n", .{});

    // Try to allocate in migrating shard
    if (manager.allocateDistributed(3, 1000)) |_| {
        std.debug.print("  ✗ Unexpected success allocating in migrating shard\n", .{});
    } else |err| {
        std.debug.print("  ✓ Correctly rejected allocation in migrating shard: {}\n", .{err});
    }

    // Set shard 4 to offline
    try manager.setShardStatus(4, .offline);
    std.debug.print("  Set shard 4 to offline status\n", .{});

    if (manager.allocateDistributed(4, 1000)) |_| {
        std.debug.print("  ✗ Unexpected success allocating in offline shard\n", .{});
    } else |err| {
        std.debug.print("  ✓ Correctly rejected allocation in offline shard: {}\n", .{err});
    }

    // Restore shard statuses
    try manager.setShardStatus(3, .active);
    try manager.setShardStatus(4, .active);
    std.debug.print("  Restored shard statuses to active\n", .{});
    std.debug.print("\n", .{});

    // === Demonstrate distributed garbage collection ===
    std.debug.print("=== Distributed Garbage Collection ===\n", .{});

    // Create and delete some objects to generate garbage
    const garbage_id = [_]u8{99} ** 16;
    var garbage_obj = try Object.init(allocator, 99, garbage_id, "Garbage object", &[_]u8{}, &[_]ObjRef{});
    defer garbage_obj.deinit();

    const garbage_data = try garbage_obj.toBytes(allocator);
    defer allocator.free(garbage_data);

    const garbage_ref = try manager.allocateDistributed(1, @intCast(garbage_data.len));
    try arenas[0].writeGlobal(garbage_ref, garbage_data);

    // Delete the object to create garbage
    try arenas[0].deleteGlobal(garbage_ref);
    std.debug.print("  Created garbage object in shard 1\n", .{});

    // Run distributed GC
    try manager.runDistributedGC();
    std.debug.print("  ✓ Distributed garbage collection completed\n", .{});
    std.debug.print("\n", .{});

    // === Demonstrate migration ===
    std.debug.print("=== Migration Demonstration ===\n", .{});

    // Start migration from shard 1 to shard 2
    try manager.startMigration(1, 2);
    std.debug.print("  Started migration from shard 1 to shard 2\n", .{});

    // Check migration progress
    if (manager.getMigrationProgress(1)) |progress| {
        std.debug.print("  Migration progress: {d:.1}%\n", .{progress});
    }

    // Perform incremental migration
    try manager.performIncrementalMigration(5);
    std.debug.print("  Performed incremental migration (batch size: 5)\n", .{});

    if (manager.getMigrationProgress(1)) |progress| {
        std.debug.print("  Updated migration progress: {d:.1}%\n", .{progress});
    }
    std.debug.print("\n", .{});

    // === Demonstrate rebalancing ===
    std.debug.print("=== Rebalancing Demonstration ===\n", .{});

    // Create load imbalance by allocating many objects in shard 1
    for (0..10) |i| {
        const imbalance_id = [_]u8{@intCast(i + 100)} ** 16;
        const content = try std.fmt.allocPrint(allocator, "Imbalance object {}", .{i});
        defer allocator.free(content);

        var imbalance_obj = try Object.init(allocator, @intCast(i + 100), imbalance_id, content, &[_]u8{}, &[_]ObjRef{});
        defer imbalance_obj.deinit();

        const data = try imbalance_obj.toBytes(allocator);
        defer allocator.free(data);

        const ref = try manager.allocateDistributed(1, @intCast(data.len));
        try arenas[0].writeGlobal(ref, data);
    }

    std.debug.print("  Created load imbalance (10 extra objects in shard 1)\n", .{});

    // Trigger rebalancing
    try manager.rebalanceShards();
    std.debug.print("  ✓ Rebalancing initiated\n", .{});
    std.debug.print("\n", .{});

    // === Show distributed statistics ===
    std.debug.print("=== Distributed Statistics ===\n", .{});

    var stats = try manager.getDistributedStats(allocator);
    defer stats.deinit(allocator);

    var total_objects: u64 = 0;
    var total_used: usize = 0;
    var total_size: usize = 0;

    for (stats.items) |stat| {
        std.debug.print("  Shard {}: {} | {} objects | {d:.1} MB used / {d:.1} MB total\n", .{
            stat.shard_id,
            stat.status,
            stat.object_count,
            @as(f64, @floatFromInt(stat.used_size)) / (1024.0 * 1024.0),
            @as(f64, @floatFromInt(stat.total_size)) / (1024.0 * 1024.0),
        });

        total_objects += stat.object_count;
        total_used += stat.used_size;
        total_size += stat.total_size;
    }

    std.debug.print("\n", .{});
    std.debug.print("  Total: {} objects | {d:.1} MB used / {d:.1} MB total\n", .{
        total_objects,
        @as(f64, @floatFromInt(total_used)) / (1024.0 * 1024.0),
        @as(f64, @floatFromInt(total_size)) / (1024.0 * 1024.0),
    });
    std.debug.print("  Utilization: {d:.1}%\n", .{
        (@as(f64, @floatFromInt(total_used)) / @as(f64, @floatFromInt(total_size))) * 100.0,
    });
    std.debug.print("\n", .{});

    // === Demonstrate error handling ===
    std.debug.print("=== Error Handling ===\n", .{});

    // Try operations on non-existent shard
    if (manager.allocateDistributed(99, 1000)) |_| {
        std.debug.print("  ✗ Unexpected success with non-existent shard\n", .{});
    } else |err| {
        std.debug.print("  ✓ Correctly handled non-existent shard: {}\n", .{err});
    }

    // Try cross-shard resolution with invalid reference
    const invalid_ref = GlobalObjRef.init(99, 1024, 0);
    if (manager.resolveCrossShardRef(invalid_ref)) |obj| {
        if (obj) |o| {
            var mutable_o = o;
            mutable_o.deinit();
        }
        std.debug.print("  ✗ Unexpected success resolving invalid reference\n", .{});
    } else |err| {
        std.debug.print("  ✓ Correctly handled invalid cross-shard reference: {}\n", .{err});
    }

    // Navigation to non-existent shard
    const nav_result = manager.navigateDistributed(invalid_ref);
    if (nav_result.isRemote()) {
        const remote_info = nav_result.getRemoteInfo().?;
        std.debug.print("  ✓ Correctly identified remote reference: shard {}\n", .{remote_info.shard_id});
    }
    std.debug.print("\n", .{});

    // Clean up objects
    for (&objects) |*obj| {
        obj.deinit();
    }

    std.debug.print("=== Demo Complete ===\n", .{});
    std.debug.print("Key distributed arena management features demonstrated:\n", .{});
    std.debug.print("  ✓ Shard-aware arena allocation and management\n", .{});
    std.debug.print("  ✓ Cross-shard object reference resolution\n", .{});
    std.debug.print("  ✓ Distributed garbage collection coordination\n", .{});
    std.debug.print("  ✓ Arena rebalancing during shard migration\n", .{});
    std.debug.print("  ✓ Co-location support across distributed shards\n", .{});
    std.debug.print("  ✓ Comprehensive error handling and status management\n", .{});
    std.debug.print("  ✓ Real-time statistics and monitoring\n", .{});
}
