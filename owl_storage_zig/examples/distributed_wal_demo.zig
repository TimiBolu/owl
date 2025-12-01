//! Distributed WAL Demo
//!
//! This example demonstrates the distributed WAL functionality including:
//! - Shard-specific WAL management
//! - Cross-shard transaction coordination
//! - WAL replication for high availability
//! - Shard synchronization

const std = @import("std");
const print = std.debug.print;
const Allocator = std.mem.Allocator;

const Arena = @import("../src/arena.zig").Arena;
const WAL = @import("../src/wal.zig").WAL;
const Object = @import("../src/object.zig").Object;
const ObjRef = @import("../src/objref.zig").ObjRef;
const GlobalObjRef = @import("../src/global_objref.zig").GlobalObjRef;
const ShardId = @import("../src/global_objref.zig").ShardId;

const DistributedWAL = @import("../src/distributed_wal.zig");
const ShardWALManager = DistributedWAL.ShardWALManager;
const WALReplicationManager = DistributedWAL.WALReplicationManager;
const ShardSynchronizationManager = DistributedWAL.ShardSynchronizationManager;
const DistributedWALEntry = DistributedWAL.DistributedWALEntry;
const DistributedEntryType = DistributedWAL.DistributedEntryType;
const TransactionState = DistributedWAL.TransactionState;

/// Helper to create demo arena
fn createDemoArena(allocator: Allocator, name: []const u8, shard_id: ShardId) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, name });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 2 * 1024 * 1024, shard_id); // 2MB
    return .{ .arena = arena, .dir = tmp };
}

/// Helper to create demo WAL
fn createDemoWAL(allocator: Allocator, name: []const u8) !struct { wal: WAL, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const wal_path = try std.fs.path.join(allocator, &[_][]const u8{ path, name });
    defer allocator.free(wal_path);

    const wal = try WAL.init(allocator, wal_path);
    return .{ .wal = wal, .dir = tmp };
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    print("=== Distributed WAL Demo ===\n\n", .{});

    // Create multiple shards
    const num_shards = 3;
    var arenas: [num_shards]@TypeOf(try createDemoArena(allocator, "test", 0)) = undefined;
    var wals: [num_shards]@TypeOf(try createDemoWAL(allocator, "test")) = undefined;
    var shard_managers: [num_shards]ShardWALManager = undefined;

    // Initialize all shards
    for (0..num_shards) |i| {
        const arena_name = try std.fmt.allocPrint(allocator, "demo_arena_{}.dat", .{i});
        defer allocator.free(arena_name);
        arenas[i] = try createDemoArena(allocator, arena_name, @intCast(i));

        const wal_name = try std.fmt.allocPrint(allocator, "demo_wal_{}", .{i});
        defer allocator.free(wal_name);
        wals[i] = try createDemoWAL(allocator, wal_name);

        shard_managers[i] = ShardWALManager.init(allocator, &wals[i].wal, @intCast(i));

        print("Initialized shard {} with arena and WAL\n", .{i});
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

    print("\n=== Setting up Replication ===\n", .{});

    // Create replication managers
    var replication_managers: [num_shards]WALReplicationManager = undefined;
    for (0..num_shards) |i| {
        replication_managers[i] = WALReplicationManager.init(allocator, &shard_managers[i]);

        // Set up replication topology (each shard replicates to the next)
        const next_shard = (i + 1) % num_shards;
        try replication_managers[i].addReplica(@intCast(next_shard));

        print("Shard {} replicating to shard {}\n", .{ i, next_shard });
    }

    defer {
        for (0..num_shards) |i| {
            replication_managers[i].deinit();
        }
    }

    print("\n=== Setting up Synchronization ===\n", .{});

    // Create synchronization managers
    var sync_managers: [num_shards]ShardSynchronizationManager = undefined;
    for (0..num_shards) |i| {
        sync_managers[i] = ShardSynchronizationManager.init(allocator, &shard_managers[i]);

        // Start sync with other shards
        for (0..num_shards) |j| {
            if (i != j) {
                try sync_managers[i].startSynchronization(@intCast(j), 0);
                print("Started sync from shard {} to shard {}\n", .{ i, j });
            }
        }
    }

    defer {
        for (0..num_shards) |i| {
            sync_managers[i].deinit();
        }
    }

    print("\n=== Creating Test Objects ===\n", .{});

    // Create test objects in each shard
    var object_refs: [num_shards]GlobalObjRef = undefined;
    for (0..num_shards) |i| {
        const id = [_]u8{@intCast(i + 1)} ** 16;
        const data_content = try std.fmt.allocPrint(allocator, "Object data for shard {}", .{i});
        defer allocator.free(data_content);

        var obj = try Object.init(allocator, @intCast(i + 1), id, &[_]u8{@intCast(i)}, data_content, &[_]ObjRef{});
        defer obj.deinit();

        const obj_data = try obj.toBytes(allocator);
        defer allocator.free(obj_data);

        const objref = try arenas[i].arena.alloc(@intCast(obj_data.len));
        try arenas[i].arena.write(objref, obj_data);

        object_refs[i] = GlobalObjRef.fromLocal(objref, @intCast(i));

        print("Created object {} in shard {} (size: {} bytes)\n", .{ i + 1, i, obj_data.len });
    }

    print("\n=== Distributed Transaction Demo ===\n", .{});

    // Demonstrate distributed transaction
    const participants = [_]ShardId{ 1, 2 };
    const coordinator_txn_id = try shard_managers[0].beginDistributedTransaction(&participants);
    print("Started distributed transaction {} on coordinator shard 0\n", .{coordinator_txn_id});

    // Write cross-shard operations
    for (1..num_shards) |i| {
        const operation_data = try std.fmt.allocPrint(allocator, "Cross-shard operation to shard {}", .{i});
        defer allocator.free(operation_data);

        try shard_managers[0].writeCrossShardOperation(
            coordinator_txn_id,
            .insert,
            object_refs[i],
            @intCast(i),
            operation_data,
        );

        print("Wrote cross-shard operation to shard {}\n", .{i});
    }

    // Simulate participant transactions
    var participant_txn_ids: [num_shards - 1]@TypeOf(coordinator_txn_id) = undefined;
    for (1..num_shards) |i| {
        const coordinator_participant = [_]ShardId{0};
        participant_txn_ids[i - 1] = try shard_managers[i].beginDistributedTransaction(&coordinator_participant);
        print("Started participant transaction {} on shard {}\n", .{ participant_txn_ids[i - 1], i });
    }

    print("\n=== Two-Phase Commit Protocol ===\n", .{});

    // Phase 1: Prepare
    print("Phase 1: Prepare\n", .{});
    const coord_prepared = try shard_managers[0].prepareDistributedTransaction(coordinator_txn_id);
    print("  Coordinator prepared: {}\n", .{coord_prepared});

    for (participant_txn_ids, 1..) |txn_id, i| {
        const prepared = try shard_managers[i].prepareDistributedTransaction(txn_id);
        print("  Participant {} prepared: {}\n", .{ i, prepared });
    }

    // Phase 2: Commit
    print("Phase 2: Commit\n", .{});
    const coord_committed = try shard_managers[0].commitDistributedTransaction(coordinator_txn_id);
    print("  Coordinator committed: {}\n", .{coord_committed});

    for (participant_txn_ids, 1..) |txn_id, i| {
        const committed = try shard_managers[i].commitDistributedTransaction(txn_id);
        print("  Participant {} committed: {}\n", .{ i, committed });
    }

    print("\n=== Replication Demo ===\n", .{});

    // Demonstrate replication
    for (0..num_shards) |i| {
        var entry = try DistributedWALEntry.init(
            allocator,
            @intCast(i + 10),
            .insert,
            object_refs[i],
            @intCast(i),
            @intCast(i),
            0, // no transaction for this demo
            0,
            "Replication demo entry",
        );
        defer entry.deinit(allocator);

        try replication_managers[i].replicateEntry(&entry);

        const next_shard = (i + 1) % num_shards;
        const lag = replication_managers[i].getReplicationLag(@intCast(next_shard));
        print("Replicated entry from shard {} to shard {} (lag: {})\n", .{ i, next_shard, lag.? });
    }

    print("\n=== Synchronization Demo ===\n", .{});

    // Demonstrate synchronization progress
    for (0..num_shards) |i| {
        for (0..num_shards) |j| {
            if (i != j) {
                try sync_managers[i].updateSyncProgress(@intCast(j), @intCast(i * 10 + j));

                const status = sync_managers[i].getSyncStatus(@intCast(j));
                if (status) |s| {
                    print("Sync from shard {} to shard {}: LSN {} (rate: {d:.2} LSN/s)\n", .{ i, j, s.last_synced_lsn, s.syncRate() });
                }
            }
        }
    }

    // Demonstrate catchup sync
    print("\nPerforming catchup sync from shard 0 to shard 1...\n", .{});
    try sync_managers[0].performCatchupSync(1, 100);
    const catchup_status = sync_managers[0].getSyncStatus(1);
    if (catchup_status) |s| {
        print("Catchup complete: LSN {} (duration: {}s)\n", .{ s.last_synced_lsn, s.duration() });
    }

    print("\n=== Final Statistics ===\n", .{});

    // Show final statistics
    for (0..num_shards) |i| {
        const arena_stats = arenas[i].arena.stats();
        const wal_stats = wals[i].wal.getStats();

        print("Shard {}:\n", .{i});
        print("  Arena: {} objects, {} bytes used\n", .{ arena_stats.object_count, arena_stats.used_size });
        print("  WAL: {} entries, last LSN {}\n", .{ wal_stats.entry_count, wal_stats.last_lsn });

        // Check active transactions
        const active_txns = shard_managers[i].active_transactions.count();
        print("  Active transactions: {}\n", .{active_txns});
    }

    print("\n=== Cleanup ===\n", .{});

    // Cleanup transactions
    shard_managers[0].cleanupTransaction(coordinator_txn_id);
    print("Cleaned up coordinator transaction {}\n", .{coordinator_txn_id});

    for (participant_txn_ids, 1..) |txn_id, i| {
        shard_managers[i].cleanupTransaction(txn_id);
        print("Cleaned up participant transaction {} on shard {}\n", .{ txn_id, i });
    }

    // Stop synchronization
    for (0..num_shards) |i| {
        for (0..num_shards) |j| {
            if (i != j) {
                sync_managers[i].stopSynchronization(@intCast(j));
            }
        }
    }

    print("\nDistributed WAL demo completed successfully!\n", .{});
}
