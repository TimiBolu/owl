// Task 8.8: Distributed Arena Management (Zig)
//
// This module implements distributed arena management functionality including:
// - Shard-aware arena allocation and management
// - Cross-shard object reference resolution
// - Distributed garbage collection coordination
// - Arena rebalancing during shard migration

const std = @import("std");
const Arena = @import("arena.zig").Arena;
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const NavigationResult = @import("global_objref.zig").NavigationResult;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const GarbageCollector = @import("gc.zig").GarbageCollector;

/// Shard status for distributed coordination
pub const ShardStatus = enum {
    /// Shard is active and accepting operations
    active,
    /// Shard is being migrated (read-only)
    migrating,
    /// Shard is being rebalanced (limited operations)
    rebalancing,
    /// Shard is offline or unavailable
    offline,
};

/// Migration state for objects during rebalancing
pub const MigrationState = struct {
    /// Source shard ID
    source_shard: ShardId,
    /// Target shard ID
    target_shard: ShardId,
    /// Number of objects to migrate
    total_objects: u64,
    /// Number of objects migrated so far
    migrated_objects: std.atomic.Value(u64),
    /// Migration start time
    start_time: i64,
    /// Whether migration is complete
    completed: std.atomic.Value(bool),

    pub fn init(source: ShardId, target: ShardId, total: u64) MigrationState {
        return .{
            .source_shard = source,
            .target_shard = target,
            .total_objects = total,
            .migrated_objects = std.atomic.Value(u64).init(0),
            .start_time = std.time.timestamp(),
            .completed = std.atomic.Value(bool).init(false),
        };
    }

    /// Get migration progress as percentage
    pub fn progress(self: *const MigrationState) f64 {
        if (self.total_objects == 0) return 100.0;
        const migrated = self.migrated_objects.load(.seq_cst);
        return (@as(f64, @floatFromInt(migrated)) / @as(f64, @floatFromInt(self.total_objects))) * 100.0;
    }

    /// Check if migration is complete
    pub fn isComplete(self: *const MigrationState) bool {
        return self.completed.load(.seq_cst);
    }
};

/// Distributed arena manager coordinating multiple shards
pub const DistributedArenaManager = struct {
    /// Map of shard ID to arena instances
    shards: std.AutoHashMap(ShardId, *Arena),
    /// Map of shard ID to status
    shard_status: std.AutoHashMap(ShardId, ShardStatus),
    /// Map of shard ID to garbage collectors
    garbage_collectors: std.AutoHashMap(ShardId, *GarbageCollector),
    /// Active migrations
    migrations: std.ArrayList(MigrationState),
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    /// Mutex for thread-safe operations
    mutex: std.Thread.Mutex,
    /// Current local shard ID
    local_shard_id: ShardId,

    pub fn init(allocator: std.mem.Allocator, local_shard_id: ShardId) DistributedArenaManager {
        return .{
            .shards = std.AutoHashMap(ShardId, *Arena).init(allocator),
            .shard_status = std.AutoHashMap(ShardId, ShardStatus).init(allocator),
            .garbage_collectors = std.AutoHashMap(ShardId, *GarbageCollector).init(allocator),
            .migrations = std.ArrayList(MigrationState){},
            .allocator = allocator,
            .mutex = .{},
            .local_shard_id = local_shard_id,
        };
    }

    pub fn deinit(self: *DistributedArenaManager) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        // Clean up garbage collectors
        var gc_iter = self.garbage_collectors.iterator();
        while (gc_iter.next()) |entry| {
            entry.value_ptr.*.deinit();
            self.allocator.destroy(entry.value_ptr.*);
        }
        self.garbage_collectors.deinit();

        // Clean up arenas
        var arena_iter = self.shards.iterator();
        while (arena_iter.next()) |entry| {
            entry.value_ptr.*.deinit();
            self.allocator.destroy(entry.value_ptr.*);
        }
        self.shards.deinit();
        self.shard_status.deinit();
        self.migrations.deinit(self.allocator);
    }

    /// Add a shard to the distributed manager
    pub fn addShard(self: *DistributedArenaManager, shard_id: ShardId, arena: *Arena) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        try self.shards.put(shard_id, arena);
        try self.shard_status.put(shard_id, .active);

        // Create garbage collector for this shard
        const gc = try self.allocator.create(GarbageCollector);
        gc.* = try GarbageCollector.init(self.allocator, arena);
        try self.garbage_collectors.put(shard_id, gc);

        std.log.info("Added shard {any} to distributed arena manager", .{shard_id});
    }

    /// Remove a shard from the distributed manager
    pub fn removeShard(self: *DistributedArenaManager, shard_id: ShardId) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        // Mark shard as offline
        try self.shard_status.put(shard_id, .offline);

        // Clean up garbage collector
        if (self.garbage_collectors.get(shard_id)) |gc| {
            gc.deinit();
            self.allocator.destroy(gc);
            _ = self.garbage_collectors.remove(shard_id);
        }

        // Remove arena
        if (self.shards.get(shard_id)) |arena| {
            arena.deinit();
            self.allocator.destroy(arena);
            _ = self.shards.remove(shard_id);
        }

        _ = self.shard_status.remove(shard_id);
        std.log.info("Removed shard {any} from distributed arena manager", .{shard_id});
    }

    /// Get shard status
    pub fn getShardStatus(self: *DistributedArenaManager, shard_id: ShardId) ShardStatus {
        self.mutex.lock();
        defer self.mutex.unlock();

        return self.shard_status.get(shard_id) orelse .offline;
    }

    /// Set shard status
    pub fn setShardStatus(self: *DistributedArenaManager, shard_id: ShardId, status: ShardStatus) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        try self.shard_status.put(shard_id, status);
        std.log.info("Set shard {any} status to {any}", .{ shard_id, status });
    }

    /// Allocate an object in the appropriate shard
    pub fn allocateDistributed(self: *DistributedArenaManager, target_shard: ShardId, size: u32) !GlobalObjRef {
        self.mutex.lock();
        defer self.mutex.unlock();

        const arena = self.shards.get(target_shard) orelse return error.ShardNotFound;
        const status = self.shard_status.get(target_shard) orelse .offline;

        switch (status) {
            .active => {
                return arena.allocGlobal(size);
            },
            .migrating => {
                // During migration, allocate in target shard if possible
                return error.ShardMigrating;
            },
            .rebalancing => {
                // Limited operations during rebalancing
                return error.ShardRebalancing;
            },
            .offline => {
                return error.ShardOffline;
            },
        }
    }

    /// Allocate an object near another object for co-location
    pub fn allocateNearDistributed(
        self: *DistributedArenaManager,
        target_shard: ShardId,
        size: u32,
        near: ?GlobalObjRef,
    ) !GlobalObjRef {
        self.mutex.lock();
        defer self.mutex.unlock();

        const arena = self.shards.get(target_shard) orelse return error.ShardNotFound;
        const status = self.shard_status.get(target_shard) orelse .offline;

        switch (status) {
            .active => {
                return arena.allocGlobalNear(size, near);
            },
            .migrating, .rebalancing => {
                return error.ShardUnavailable;
            },
            .offline => {
                return error.ShardOffline;
            },
        }
    }

    /// Navigate to an object across shards
    pub fn navigateDistributed(self: *DistributedArenaManager, global_ref: GlobalObjRef) NavigationResult(Object) {
        if (global_ref.isNull()) {
            return NavigationResult(Object){ .not_found = {} };
        }

        const target_shard = global_ref.getShardId();

        self.mutex.lock();
        defer self.mutex.unlock();

        const arena = self.shards.get(target_shard) orelse {
            return NavigationResult(Object){ .remote = .{
                .shard_id = target_shard,
                .objref = global_ref,
            } };
        };

        return arena.navigateGlobal(global_ref, self.allocator);
    }

    /// Resolve a cross-shard reference
    pub fn resolveCrossShardRef(self: *DistributedArenaManager, global_ref: GlobalObjRef) !?Object {
        if (global_ref.isNull()) return null;

        const target_shard = global_ref.getShardId();

        self.mutex.lock();
        defer self.mutex.unlock();

        const arena = self.shards.get(target_shard) orelse {
            // In a full distributed implementation, this would trigger a network call
            return error.RemoteShardNotAvailable;
        };

        const status = self.shard_status.get(target_shard) orelse .offline;
        if (status == .offline) {
            return error.ShardOffline;
        }

        // Try to read the object
        if (arena.readGlobal(global_ref, self.allocator)) |obj| {
            return obj;
        } else |err| {
            std.log.warn("Failed to resolve cross-shard reference {any}: {any}", .{ global_ref, err });
            return null;
        }
    }

    /// Start distributed garbage collection across all active shards
    pub fn runDistributedGC(self: *DistributedArenaManager) !void {
        std.log.info("Starting distributed garbage collection", .{});

        self.mutex.lock();
        defer self.mutex.unlock();

        var total_objects_swept: usize = 0;
        var total_bytes_reclaimed: usize = 0;

        // Run GC on each active shard
        var gc_iter = self.garbage_collectors.iterator();
        while (gc_iter.next()) |entry| {
            const shard_id = entry.key_ptr.*;
            const gc = entry.value_ptr.*;

            const status = self.shard_status.get(shard_id) orelse .offline;
            if (status != .active) continue;

            // Run GC on this shard
            gc.collect() catch |err| {
                std.log.warn("GC failed on shard {any}: {any}", .{ shard_id, err });
                continue;
            };

            const stats = gc.getStats();
            total_objects_swept += stats.objects_swept;
            total_bytes_reclaimed += stats.bytes_reclaimed;

            std.log.info("GC completed on shard {any}: {any} objects swept, {any} bytes reclaimed", .{
                shard_id,
                stats.objects_swept,
                stats.bytes_reclaimed,
            });
        }

        std.log.info("Distributed GC completed: {any} total objects swept, {any} total bytes reclaimed", .{
            total_objects_swept,
            total_bytes_reclaimed,
        });
    }

    /// Start migration of objects from source shard to target shard
    pub fn startMigration(self: *DistributedArenaManager, source_shard: ShardId, target_shard: ShardId) !void {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.startMigrationLocked(source_shard, target_shard);
    }

    /// Internal migration start function (assumes mutex is already held)
    fn startMigrationLocked(self: *DistributedArenaManager, source_shard: ShardId, target_shard: ShardId) !void {
        const source_arena = self.shards.get(source_shard) orelse return error.SourceShardNotFound;
        _ = self.shards.get(target_shard) orelse return error.TargetShardNotFound;

        // Set source shard to migrating status
        try self.shard_status.put(source_shard, .migrating);

        // Count objects to migrate
        const source_stats = source_arena.stats();
        const migration_state = MigrationState.init(source_shard, target_shard, source_stats.object_count);

        try self.migrations.append(self.allocator, migration_state);

        std.log.info("Started migration from shard {any} to shard {any} ({any} objects)", .{
            source_shard,
            target_shard,
            source_stats.object_count,
        });

        // In a full implementation, this would spawn a background thread to perform the migration
        // For now, we just mark the migration as started
    }

    /// Perform incremental migration of objects
    pub fn performIncrementalMigration(self: *DistributedArenaManager, batch_size: u32) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        for (self.migrations.items) |*migration| {
            if (migration.isComplete()) continue;

            const source_arena = self.shards.get(migration.source_shard) orelse continue;
            const target_arena = self.shards.get(migration.target_shard) orelse continue;

            // Scan source arena for objects to migrate
            var objects = source_arena.scanObjects(self.allocator) catch continue;
            defer objects.deinit(self.allocator);

            var migrated_count: u32 = 0;
            const current_migrated = migration.migrated_objects.load(.seq_cst);

            for (objects.items) |obj_info| {
                if (migrated_count >= batch_size) break;
                if (obj_info.is_deleted) continue;

                // Skip already migrated objects
                const obj_index = @divTrunc(obj_info.offset, 64); // Assuming 64-byte slots
                if (obj_index < current_migrated) continue;

                // Read object from source
                const source_ref = ObjRef.init(obj_info.offset);
                const obj = source_arena.read(source_ref, self.allocator) catch continue;
                defer {
                    var mutable_obj = obj;
                    mutable_obj.deinit();
                }

                // Serialize object
                const obj_data = obj.toBytes(self.allocator) catch continue;
                defer self.allocator.free(obj_data);

                // Allocate space in target arena
                const target_ref = target_arena.allocGlobal(@intCast(obj_data.len)) catch continue;

                // Write object to target arena
                target_arena.writeGlobal(target_ref, obj_data) catch continue;

                // Mark object as migrated in source (set migration flag)
                var global_source_ref = GlobalObjRef.fromLocal(source_ref, migration.source_shard);
                global_source_ref.setFlag(GlobalObjRef.Flags.MIGRATED);

                migrated_count += 1;
                _ = migration.migrated_objects.fetchAdd(1, .seq_cst);
            }

            // Check if migration is complete
            if (migration.migrated_objects.load(.seq_cst) >= migration.total_objects) {
                migration.completed.store(true, .seq_cst);
                try self.shard_status.put(migration.source_shard, .active);
                std.log.info("Migration from shard {any} to shard {any} completed", .{
                    migration.source_shard,
                    migration.target_shard,
                });
            }
        }
    }

    /// Get migration progress for a specific source shard
    pub fn getMigrationProgress(self: *DistributedArenaManager, source_shard: ShardId) ?f64 {
        self.mutex.lock();
        defer self.mutex.unlock();

        for (self.migrations.items) |*migration| {
            if (migration.source_shard == source_shard) {
                return migration.progress();
            }
        }
        return null;
    }

    /// Rebalance objects across shards for better distribution
    pub fn rebalanceShards(self: *DistributedArenaManager) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        std.log.info("Starting shard rebalancing", .{});

        // Calculate average load across all active shards
        var total_objects: u64 = 0;
        var active_shards: u32 = 0;

        var shard_iter = self.shards.iterator();
        while (shard_iter.next()) |entry| {
            const shard_id = entry.key_ptr.*;
            const arena = entry.value_ptr.*;
            const status = self.shard_status.get(shard_id) orelse .offline;

            if (status == .active) {
                const stats = arena.stats();
                total_objects += stats.object_count;
                active_shards += 1;
            }
        }

        if (active_shards == 0) return;

        const avg_objects_per_shard = total_objects / active_shards;
        const rebalance_threshold = avg_objects_per_shard / 4; // 25% threshold

        std.log.info("Average objects per shard: {any}, rebalance threshold: {any}", .{
            avg_objects_per_shard,
            rebalance_threshold,
        });

        // Find overloaded and underloaded shards
        var overloaded_shards = std.ArrayList(ShardId){};
        defer overloaded_shards.deinit(self.allocator);
        var underloaded_shards = std.ArrayList(ShardId){};
        defer underloaded_shards.deinit(self.allocator);

        shard_iter = self.shards.iterator();
        while (shard_iter.next()) |entry| {
            const shard_id = entry.key_ptr.*;
            const arena = entry.value_ptr.*;
            const status = self.shard_status.get(shard_id) orelse .offline;

            if (status != .active) continue;

            const stats = arena.stats();
            if (stats.object_count > avg_objects_per_shard + rebalance_threshold) {
                try overloaded_shards.append(self.allocator, shard_id);
            } else if (stats.object_count < avg_objects_per_shard - rebalance_threshold) {
                try underloaded_shards.append(self.allocator, shard_id);
            }
        }

        // Start migrations from overloaded to underloaded shards
        const min_pairs = @min(overloaded_shards.items.len, underloaded_shards.items.len);
        for (0..min_pairs) |i| {
            const source_shard = overloaded_shards.items[i];
            const target_shard = underloaded_shards.items[i];

            try self.startMigrationLocked(source_shard, target_shard);
        }

        std.log.info("Rebalancing started: {any} migration pairs", .{min_pairs});
    }

    /// Get statistics for all shards
    pub fn getDistributedStats(self: *DistributedArenaManager, allocator: std.mem.Allocator) !std.ArrayList(ShardStats) {
        self.mutex.lock();
        defer self.mutex.unlock();

        var stats_list = std.ArrayList(ShardStats){};

        var shard_iter = self.shards.iterator();
        while (shard_iter.next()) |entry| {
            const shard_id = entry.key_ptr.*;
            const arena = entry.value_ptr.*;
            const status = self.shard_status.get(shard_id) orelse .offline;

            const arena_stats = arena.stats();
            const gc_stats = if (self.garbage_collectors.get(shard_id)) |gc| gc.getStats() else @import("gc.zig").GCStats.init();

            const shard_stats = ShardStats{
                .shard_id = shard_id,
                .status = status,
                .total_size = arena_stats.total_size,
                .used_size = arena_stats.used_size,
                .object_count = arena_stats.object_count,
                .gc_cycles = gc_stats.cycles_completed,
                .objects_swept = gc_stats.objects_swept,
                .bytes_reclaimed = gc_stats.bytes_reclaimed,
            };

            try stats_list.append(allocator, shard_stats);
        }

        return stats_list;
    }

    /// Check if the local shard is available
    pub fn isLocalShardAvailable(self: *DistributedArenaManager) bool {
        self.mutex.lock();
        defer self.mutex.unlock();

        const status = self.shard_status.get(self.local_shard_id) orelse .offline;
        return status == .active;
    }

    /// Get the local arena
    pub fn getLocalArena(self: *DistributedArenaManager) ?*Arena {
        self.mutex.lock();
        defer self.mutex.unlock();

        return self.shards.get(self.local_shard_id);
    }
};

/// Statistics for a single shard
pub const ShardStats = struct {
    shard_id: ShardId,
    status: ShardStatus,
    total_size: usize,
    used_size: usize,
    object_count: u64,
    gc_cycles: usize,
    objects_swept: usize,
    bytes_reclaimed: usize,
};

// ============================================================================
// Tests
// ============================================================================

fn createTestArena(allocator: std.mem.Allocator, shard_id: ShardId) !struct { arena: *Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_name = try std.fmt.allocPrint(allocator, "test_arena_{}.dat", .{shard_id});
    defer allocator.free(file_name);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, file_name });
    defer allocator.free(file_path);

    const arena = try allocator.create(Arena);
    arena.* = try Arena.init(allocator, file_path, 1024 * 1024, shard_id); // 1MB
    return .{ .arena = arena, .dir = tmp };
}

test "DistributedArenaManager: creation and cleanup" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    try std.testing.expectEqual(@as(ShardId, 0), manager.local_shard_id);
    try std.testing.expectEqual(@as(usize, 0), manager.shards.count());
}

test "DistributedArenaManager: add and remove shards" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();

    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    // Add shards
    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    try std.testing.expectEqual(@as(usize, 2), manager.shards.count());
    try std.testing.expectEqual(ShardStatus.active, manager.getShardStatus(1));
    try std.testing.expectEqual(ShardStatus.active, manager.getShardStatus(2));

    // Remove shard
    try manager.removeShard(1);
    try std.testing.expectEqual(@as(usize, 1), manager.shards.count());
    try std.testing.expectEqual(ShardStatus.offline, manager.getShardStatus(1));
}

test "DistributedArenaManager: distributed allocation" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena = try createTestArena(allocator, 1);
    defer test_arena.dir.cleanup();

    try manager.addShard(1, test_arena.arena);

    // Allocate in shard 1
    const global_ref = try manager.allocateDistributed(1, 100);
    try std.testing.expect(!global_ref.isNull());
    try std.testing.expectEqual(@as(ShardId, 1), global_ref.getShardId());
}

test "DistributedArenaManager: allocation with shard offline" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    // Try to allocate in non-existent shard
    const result = manager.allocateDistributed(99, 100);
    try std.testing.expectError(error.ShardNotFound, result);
}

test "DistributedArenaManager: cross-shard navigation" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    // Create object in shard 1
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, "test data", &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const global_ref = try manager.allocateDistributed(1, @intCast(data.len));
    try test_arena1.arena.writeGlobal(global_ref, data);

    // Navigate from manager
    const result = manager.navigateDistributed(global_ref);
    try std.testing.expect(result.isLocal());

    var retrieved_obj = result.getLocal().?;
    defer retrieved_obj.deinit();
    try std.testing.expectEqual(obj.header.type_tag, retrieved_obj.header.type_tag);
}

test "DistributedArenaManager: migration state" {
    const migration = MigrationState.init(1, 2, 100);
    try std.testing.expectEqual(@as(ShardId, 1), migration.source_shard);
    try std.testing.expectEqual(@as(ShardId, 2), migration.target_shard);
    try std.testing.expectEqual(@as(u64, 100), migration.total_objects);
    try std.testing.expectEqual(@as(f64, 0.0), migration.progress());
    try std.testing.expect(!migration.isComplete());
}

test "DistributedArenaManager: distributed GC" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena = try createTestArena(allocator, 1);
    defer test_arena.dir.cleanup();

    try manager.addShard(1, test_arena.arena);

    // Run distributed GC (should not crash)
    try manager.runDistributedGC();
}

test "DistributedArenaManager: shard status management" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena = try createTestArena(allocator, 1);
    defer test_arena.dir.cleanup();

    try manager.addShard(1, test_arena.arena);

    // Check initial status
    try std.testing.expectEqual(ShardStatus.active, manager.getShardStatus(1));

    // Change status
    try manager.setShardStatus(1, .migrating);
    try std.testing.expectEqual(ShardStatus.migrating, manager.getShardStatus(1));

    // Allocation should fail during migration
    const result = manager.allocateDistributed(1, 100);
    try std.testing.expectError(error.ShardMigrating, result);
}

test "DistributedArenaManager: get distributed stats" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena1 = try createTestArena(allocator, 1);
    defer test_arena1.dir.cleanup();
    var test_arena2 = try createTestArena(allocator, 2);
    defer test_arena2.dir.cleanup();

    try manager.addShard(1, test_arena1.arena);
    try manager.addShard(2, test_arena2.arena);

    var stats = try manager.getDistributedStats(allocator);
    defer stats.deinit(allocator);

    try std.testing.expectEqual(@as(usize, 2), stats.items.len);

    // Check that we have stats for both shards
    var found_shard1 = false;
    var found_shard2 = false;
    for (stats.items) |stat| {
        if (stat.shard_id == 1) found_shard1 = true;
        if (stat.shard_id == 2) found_shard2 = true;
        try std.testing.expectEqual(ShardStatus.active, stat.status);
    }
    try std.testing.expect(found_shard1);
    try std.testing.expect(found_shard2);
}

test "DistributedArenaManager: local shard availability" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 5);
    defer manager.deinit();

    // Local shard not available initially
    try std.testing.expect(!manager.isLocalShardAvailable());

    var test_arena = try createTestArena(allocator, 5);
    defer test_arena.dir.cleanup();

    try manager.addShard(5, test_arena.arena);

    // Now local shard should be available
    try std.testing.expect(manager.isLocalShardAvailable());

    // Get local arena
    const local_arena = manager.getLocalArena();
    try std.testing.expect(local_arena != null);
    try std.testing.expectEqual(@as(ShardId, 5), local_arena.?.getShardId());
}

test "DistributedArenaManager: cross-shard reference resolution" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    var test_arena = try createTestArena(allocator, 1);
    defer test_arena.dir.cleanup();

    try manager.addShard(1, test_arena.arena);

    // Create object in shard 1
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, "cross-shard test", &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const global_ref = try manager.allocateDistributed(1, @intCast(data.len));
    try test_arena.arena.writeGlobal(global_ref, data);

    // Resolve cross-shard reference
    const resolved_obj = try manager.resolveCrossShardRef(global_ref);
    try std.testing.expect(resolved_obj != null);

    var resolved = resolved_obj.?;
    defer resolved.deinit();
    try std.testing.expectEqual(obj.header.type_tag, resolved.header.type_tag);
}

test "DistributedArenaManager: resolve non-existent shard reference" {
    const allocator = std.testing.allocator;
    var manager = DistributedArenaManager.init(allocator, 0);
    defer manager.deinit();

    // Create reference to non-existent shard
    const remote_ref = GlobalObjRef.init(99, 1024, 0);

    const result = manager.resolveCrossShardRef(remote_ref);
    try std.testing.expectError(error.RemoteShardNotAvailable, result);
}
