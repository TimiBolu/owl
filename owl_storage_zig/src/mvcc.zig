//! Multi-Version Concurrency Control (MVCC)
//!
//! Implements snapshot isolation for concurrent transactions.
//! Each transaction sees a consistent snapshot of the database at its start time.
//! Multiple versions of objects are maintained to support concurrent reads and writes.

const std = @import("std");
const Allocator = std.mem.Allocator;
const ObjRef = @import("objref.zig").ObjRef;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const LSN = @import("wal.zig").LSN;
const TransactionId = @import("transaction.zig").TransactionId;

/// Version chain entry - links to previous version of an object
pub const VersionChain = struct {
    /// Reference to this version
    objref: ObjRef,
    /// LSN when this version was created
    created_lsn: LSN,
    /// LSN when this version was deleted (0 if still active)
    deleted_lsn: LSN,
    /// Transaction that created this version
    created_by: TransactionId,
    /// Transaction that deleted this version (0 if not deleted)
    deleted_by: TransactionId,
    /// Reference to previous version (null if first version)
    prev_version: ObjRef,

    pub fn init(objref: ObjRef, created_lsn: LSN, created_by: TransactionId) VersionChain {
        return VersionChain{
            .objref = objref,
            .created_lsn = created_lsn,
            .deleted_lsn = 0,
            .created_by = created_by,
            .deleted_by = 0,
            .prev_version = ObjRef.nullRef(),
        };
    }

    /// Check if this version is visible to a transaction with given snapshot LSN
    pub fn isVisibleTo(self: *const VersionChain, snapshot_lsn: LSN) bool {
        // Version must be created before snapshot
        if (self.created_lsn > snapshot_lsn) return false;

        // If deleted, deletion must be after snapshot
        if (self.deleted_lsn > 0 and self.deleted_lsn <= snapshot_lsn) return false;

        return true;
    }

    /// Mark this version as deleted
    pub fn markDeleted(self: *VersionChain, deleted_lsn: LSN, deleted_by: TransactionId) void {
        self.deleted_lsn = deleted_lsn;
        self.deleted_by = deleted_by;
    }
};

/// Snapshot - represents a consistent view of the database at a point in time
pub const Snapshot = struct {
    /// LSN at snapshot creation
    snapshot_lsn: LSN,
    /// Transaction ID that owns this snapshot
    txn_id: TransactionId,
    /// Creation timestamp
    created_at: i64,
    /// Active transactions at snapshot time (for visibility checks)
    active_txns: std.ArrayList(TransactionId),
    allocator: Allocator,

    pub fn init(allocator: Allocator, snapshot_lsn: LSN, txn_id: TransactionId, active_txns: []const TransactionId) !Snapshot {
        var active_list: std.ArrayList(TransactionId) = .{};
        try active_list.appendSlice(allocator, active_txns);

        return Snapshot{
            .snapshot_lsn = snapshot_lsn,
            .txn_id = txn_id,
            .created_at = std.time.milliTimestamp(),
            .active_txns = active_list,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *Snapshot) void {
        self.active_txns.deinit(self.allocator);
    }

    /// Check if a version is visible in this snapshot
    pub fn isVersionVisible(self: *const Snapshot, version: *const VersionChain) bool {
        // Use version chain visibility rules
        if (!version.isVisibleTo(self.snapshot_lsn)) return false;

        // Additional check: version created by active transaction at snapshot time
        // is not visible (uncommitted)
        for (self.active_txns.items) |active_txn| {
            if (version.created_by == active_txn and active_txn != self.txn_id) {
                return false;
            }
        }

        return true;
    }
};

/// Version store - manages version chains for objects
pub const VersionStore = struct {
    allocator: Allocator,
    /// Map from logical object ID to version chain head
    version_chains: std.AutoHashMap(u128, VersionChain),
    version_mutex: std.Thread.Mutex,
    /// Garbage collection watermark (versions older than this can be collected)
    gc_watermark: std.atomic.Value(LSN),

    const Self = @This();

    pub fn init(allocator: Allocator) Self {
        return Self{
            .allocator = allocator,
            .version_chains = std.AutoHashMap(u128, VersionChain).init(allocator),
            .version_mutex = .{},
            .gc_watermark = std.atomic.Value(LSN).init(0),
        };
    }

    pub fn deinit(self: *Self) void {
        self.version_chains.deinit();
    }

    /// Add a new version of an object
    pub fn addVersion(self: *Self, object_id: u128, version: VersionChain) !void {
        self.version_mutex.lock();
        defer self.version_mutex.unlock();

        // Check if there's an existing version chain
        if (self.version_chains.get(object_id)) |existing| {
            // Link to previous version
            var new_version = version;
            new_version.prev_version = existing.objref;
            try self.version_chains.put(object_id, new_version);
        } else {
            // First version
            try self.version_chains.put(object_id, version);
        }
    }

    /// Get the visible version for a snapshot
    pub fn getVisibleVersion(self: *Self, object_id: u128, snapshot: *const Snapshot) ?VersionChain {
        self.version_mutex.lock();
        defer self.version_mutex.unlock();

        var current = self.version_chains.get(object_id) orelse return null;

        // Walk version chain to find visible version
        // Note: In a full implementation, we'd need to load previous versions from storage
        // For now, we only check the current head of the chain
        if (snapshot.isVersionVisible(&current)) {
            return current;
        }

        // No visible version found
        return null;
    }

    /// Mark a version as deleted
    pub fn markDeleted(self: *Self, object_id: u128, deleted_lsn: LSN, deleted_by: TransactionId) !void {
        self.version_mutex.lock();
        defer self.version_mutex.unlock();

        if (self.version_chains.getPtr(object_id)) |version| {
            version.markDeleted(deleted_lsn, deleted_by);
        } else {
            return error.ObjectNotFound;
        }
    }

    /// Set garbage collection watermark
    pub fn setGCWatermark(self: *Self, lsn: LSN) void {
        self.gc_watermark.store(lsn, .seq_cst);
    }

    /// Get garbage collection watermark
    pub fn getGCWatermark(self: *const Self) LSN {
        return self.gc_watermark.load(.seq_cst);
    }

    /// Collect garbage versions older than watermark
    pub fn collectGarbage(self: *Self) !u64 {
        self.version_mutex.lock();
        defer self.version_mutex.unlock();

        const watermark = self.getGCWatermark();
        var collected: u64 = 0;

        var iter = self.version_chains.iterator();
        while (iter.next()) |entry| {
            const version = entry.value_ptr;

            // Can collect if deleted and deletion is before watermark
            if (version.deleted_lsn > 0 and version.deleted_lsn < watermark) {
                // In a real implementation, we'd:
                // 1. Walk the version chain
                // 2. Free old versions from arena
                // 3. Update the chain
                collected += 1;
            }
        }

        return collected;
    }

    /// Get version chain count
    pub fn getVersionCount(self: *Self) usize {
        self.version_mutex.lock();
        defer self.version_mutex.unlock();
        return self.version_chains.count();
    }
};

// Tests
test "VersionChain visibility" {
    const version = VersionChain.init(ObjRef.init(1024), 100, 1);

    // Visible to snapshot at LSN 100 or later
    try std.testing.expect(version.isVisibleTo(100));
    try std.testing.expect(version.isVisibleTo(150));

    // Not visible to snapshot before creation
    try std.testing.expect(!version.isVisibleTo(99));
}

test "VersionChain deletion visibility" {
    var version = VersionChain.init(ObjRef.init(1024), 100, 1);
    version.markDeleted(200, 2);

    // Visible between creation and deletion
    try std.testing.expect(version.isVisibleTo(100));
    try std.testing.expect(version.isVisibleTo(150));

    // Not visible after deletion
    try std.testing.expect(!version.isVisibleTo(200));
    try std.testing.expect(!version.isVisibleTo(250));
}

test "Snapshot creation" {
    const allocator = std.testing.allocator;

    const active_txns = [_]TransactionId{ 2, 3, 5 };
    var snapshot = try Snapshot.init(allocator, 100, 1, &active_txns);
    defer snapshot.deinit();

    try std.testing.expectEqual(@as(LSN, 100), snapshot.snapshot_lsn);
    try std.testing.expectEqual(@as(TransactionId, 1), snapshot.txn_id);
    try std.testing.expectEqual(@as(usize, 3), snapshot.active_txns.items.len);
}

test "Snapshot visibility with active transactions" {
    const allocator = std.testing.allocator;

    const active_txns = [_]TransactionId{2};
    var snapshot = try Snapshot.init(allocator, 100, 1, &active_txns);
    defer snapshot.deinit();

    // Version created by committed transaction - visible
    const committed_version = VersionChain.init(ObjRef.init(1024), 90, 10);
    try std.testing.expect(snapshot.isVersionVisible(&committed_version));

    // Version created by active transaction (not us) - not visible
    const active_version = VersionChain.init(ObjRef.init(2048), 95, 2);
    try std.testing.expect(!snapshot.isVersionVisible(&active_version));

    // Version created by our transaction - visible
    const our_version = VersionChain.init(ObjRef.init(3072), 95, 1);
    try std.testing.expect(snapshot.isVersionVisible(&our_version));
}

test "VersionStore add and get" {
    const allocator = std.testing.allocator;

    var store = VersionStore.init(allocator);
    defer store.deinit();

    const object_id: u128 = 12345;
    const version = VersionChain.init(ObjRef.init(1024), 100, 1);

    try store.addVersion(object_id, version);

    try std.testing.expectEqual(@as(usize, 1), store.getVersionCount());
}

test "VersionStore visible version" {
    const allocator = std.testing.allocator;

    var store = VersionStore.init(allocator);
    defer store.deinit();

    const object_id: u128 = 12345;
    const version = VersionChain.init(ObjRef.init(1024), 100, 1);

    try store.addVersion(object_id, version);

    // Create snapshot after version creation
    const active_txns = [_]TransactionId{};
    var snapshot = try Snapshot.init(allocator, 150, 2, &active_txns);
    defer snapshot.deinit();

    const visible = store.getVisibleVersion(object_id, &snapshot);
    try std.testing.expect(visible != null);
    try std.testing.expectEqual(@as(u64, 1024), visible.?.objref.getOffset());
}

test "VersionStore mark deleted" {
    const allocator = std.testing.allocator;

    var store = VersionStore.init(allocator);
    defer store.deinit();

    const object_id: u128 = 12345;
    const version = VersionChain.init(ObjRef.init(1024), 100, 1);

    try store.addVersion(object_id, version);
    try store.markDeleted(object_id, 200, 2);

    // Snapshot before deletion should see it
    const active_txns1 = [_]TransactionId{};
    var snapshot1 = try Snapshot.init(allocator, 150, 3, &active_txns1);
    defer snapshot1.deinit();

    const visible1 = store.getVisibleVersion(object_id, &snapshot1);
    try std.testing.expect(visible1 != null);

    // Snapshot after deletion should not see it
    var snapshot2 = try Snapshot.init(allocator, 250, 4, &active_txns1);
    defer snapshot2.deinit();

    const visible2 = store.getVisibleVersion(object_id, &snapshot2);
    try std.testing.expect(visible2 == null);
}

test "VersionStore garbage collection" {
    const allocator = std.testing.allocator;

    var store = VersionStore.init(allocator);
    defer store.deinit();

    // Add some versions
    const object_id1: u128 = 100;
    const version1 = VersionChain.init(ObjRef.init(1024), 50, 1);
    try store.addVersion(object_id1, version1);
    try store.markDeleted(object_id1, 100, 2);

    const object_id2: u128 = 200;
    const version2 = VersionChain.init(ObjRef.init(2048), 75, 3);
    try store.addVersion(object_id2, version2);
    try store.markDeleted(object_id2, 150, 4);

    // Set GC watermark
    store.setGCWatermark(120);

    // Collect garbage
    const collected = try store.collectGarbage();

    // Should collect version1 (deleted at 100, before watermark 120)
    // Should not collect version2 (deleted at 150, after watermark 120)
    try std.testing.expectEqual(@as(u64, 1), collected);
}

test "VersionStore multiple versions" {
    const allocator = std.testing.allocator;

    var store = VersionStore.init(allocator);
    defer store.deinit();

    const object_id: u128 = 12345;

    // Add first version
    const version1 = VersionChain.init(ObjRef.init(1024), 100, 1);
    try store.addVersion(object_id, version1);

    // Should have 1 entry
    try std.testing.expectEqual(@as(usize, 1), store.getVersionCount());

    // Snapshot at 150 should see version1
    const active_txns = [_]TransactionId{};
    var snapshot1 = try Snapshot.init(allocator, 150, 3, &active_txns);
    defer snapshot1.deinit();

    const visible1 = store.getVisibleVersion(object_id, &snapshot1);
    try std.testing.expect(visible1 != null);
    try std.testing.expectEqual(@as(u64, 1024), visible1.?.objref.getOffset());

    // Add second version (update) - this replaces the head
    const version2 = VersionChain.init(ObjRef.init(2048), 200, 2);
    try store.addVersion(object_id, version2);

    // Snapshot at 250 should see version2 (the current head)
    var snapshot2 = try Snapshot.init(allocator, 250, 4, &active_txns);
    defer snapshot2.deinit();

    const visible2 = store.getVisibleVersion(object_id, &snapshot2);
    try std.testing.expect(visible2 != null);
    try std.testing.expectEqual(@as(u64, 2048), visible2.?.objref.getOffset());

    // Note: Full version chain traversal would require loading from storage
    // This test validates the basic version visibility logic
}
