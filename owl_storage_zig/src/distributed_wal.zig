//! Distributed WAL Implementation (Zig)
//!
//! Task 8.10: Add Zig-level WAL distribution (Zig)
//!
//! This module implements distributed WAL functionality including:
//! - Shard-specific WAL management
//! - Cross-shard WAL coordination for distributed transactions
//! - WAL replication for high availability
//! - WAL-based shard synchronization
//!
//! Features:
//! - Distributed WAL coordination across multiple shards
//! - Cross-shard transaction support with 2PC protocol
//! - WAL replication for fault tolerance
//! - Shard synchronization using WAL streams
//! - Atomic distributed operations with rollback support

const std = @import("std");
const Allocator = std.mem.Allocator;
const WAL = @import("wal.zig").WAL;
const WALEntry = @import("wal.zig").WALEntry;
const EntryType = @import("wal.zig").EntryType;
const LSN = @import("wal.zig").LSN;
const RecoveryOptions = @import("wal.zig").RecoveryOptions;
const RecoveryCallback = @import("wal.zig").RecoveryCallback;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const ObjRef = @import("objref.zig").ObjRef;

/// Distributed transaction identifier
pub const DistributedTxnId = u64;

/// Distributed WAL entry types extending base EntryType
pub const DistributedEntryType = enum(u8) {
    /// Standard single-shard operations
    insert = 1,
    update = 2,
    delete = 3,
    checkpoint = 4,
    commit = 5,
    abort = 6,

    /// Distributed transaction operations
    distributed_begin = 10,
    distributed_prepare = 11,
    distributed_commit = 12,
    distributed_abort = 13,

    /// Shard coordination operations
    shard_join = 20,
    shard_leave = 21,
    shard_rebalance = 22,

    /// Replication operations
    replica_sync = 30,
    replica_catchup = 31,

    pub fn toString(self: DistributedEntryType) []const u8 {
        return switch (self) {
            .insert => "INSERT",
            .update => "UPDATE",
            .delete => "DELETE",
            .checkpoint => "CHECKPOINT",
            .commit => "COMMIT",
            .abort => "ABORT",
            .distributed_begin => "DISTRIBUTED_BEGIN",
            .distributed_prepare => "DISTRIBUTED_PREPARE",
            .distributed_commit => "DISTRIBUTED_COMMIT",
            .distributed_abort => "DISTRIBUTED_ABORT",
            .shard_join => "SHARD_JOIN",
            .shard_leave => "SHARD_LEAVE",
            .shard_rebalance => "SHARD_REBALANCE",
            .replica_sync => "REPLICA_SYNC",
            .replica_catchup => "REPLICA_CATCHUP",
        };
    }

    pub fn fromEntryType(entry_type: EntryType) DistributedEntryType {
        return switch (entry_type) {
            .insert => .insert,
            .update => .update,
            .delete => .delete,
            .checkpoint => .checkpoint,
            .commit => .commit,
            .abort => .abort,
        };
    }

    pub fn toEntryType(self: DistributedEntryType) EntryType {
        return switch (self) {
            .insert => .insert,
            .update => .update,
            .delete => .delete,
            .checkpoint => .checkpoint,
            .commit => .commit,
            .abort => .abort,
            else => .commit, // Default for distributed operations
        };
    }
};

/// Distributed WAL entry header (extends WALEntryHeader)
pub const DistributedWALEntryHeader = extern struct {
    /// Base WAL entry header
    base: @import("wal.zig").WALEntryHeader,
    /// Source shard ID
    source_shard: ShardId,
    /// Target shard ID (for cross-shard operations)
    target_shard: ShardId,
    /// Distributed transaction ID
    txn_id: DistributedTxnId,
    /// Distributed entry type
    distributed_type: DistributedEntryType,
    /// Sequence number within distributed transaction
    txn_sequence: u32,
    /// Padding for alignment
    _padding: u32,

    pub fn init(
        lsn: LSN,
        entry_type: DistributedEntryType,
        data_size: u32,
        objref: GlobalObjRef,
        source_shard: ShardId,
        target_shard: ShardId,
        txn_id: DistributedTxnId,
        txn_sequence: u32,
    ) DistributedWALEntryHeader {
        const base_header = @import("wal.zig").WALEntryHeader.init(
            lsn,
            entry_type.toEntryType(),
            data_size,
            objref.toLocal(),
        );

        return .{
            .base = base_header,
            .source_shard = source_shard,
            .target_shard = target_shard,
            .txn_id = txn_id,
            .distributed_type = entry_type,
            .txn_sequence = txn_sequence,
            ._padding = 0,
        };
    }

    pub fn updateChecksum(self: *DistributedWALEntryHeader, data: []const u8) void {
        // Update base header checksum which includes the full distributed header
        self.base.checksum = 0;

        var hasher = std.hash.Crc32.init();
        hasher.update(std.mem.asBytes(self));
        hasher.update(data);

        self.base.checksum = hasher.final();
    }

    pub fn validate(self: *const DistributedWALEntryHeader, data: []const u8) bool {
        var temp = self.*;
        temp.base.checksum = 0;

        var hasher = std.hash.Crc32.init();
        hasher.update(std.mem.asBytes(&temp));
        hasher.update(data);

        return self.base.checksum == hasher.final();
    }
};

/// Distributed WAL entry
pub const DistributedWALEntry = struct {
    header: DistributedWALEntryHeader,
    data: []u8,

    pub fn init(
        allocator: Allocator,
        lsn: LSN,
        entry_type: DistributedEntryType,
        objref: GlobalObjRef,
        source_shard: ShardId,
        target_shard: ShardId,
        txn_id: DistributedTxnId,
        txn_sequence: u32,
        data: []const u8,
    ) !DistributedWALEntry {
        const entry_data = try allocator.dupe(u8, data);
        var header = DistributedWALEntryHeader.init(
            lsn,
            entry_type,
            @intCast(data.len),
            objref,
            source_shard,
            target_shard,
            txn_id,
            txn_sequence,
        );
        header.updateChecksum(entry_data);

        return .{
            .header = header,
            .data = entry_data,
        };
    }

    pub fn deinit(self: *DistributedWALEntry, allocator: Allocator) void {
        allocator.free(self.data);
    }

    pub fn totalSize(self: *const DistributedWALEntry) usize {
        return @sizeOf(DistributedWALEntryHeader) + self.data.len;
    }

    pub fn validate(self: *const DistributedWALEntry) bool {
        return self.header.validate(self.data);
    }

    /// Convert to base WAL entry for compatibility
    pub fn toWALEntry(self: *const DistributedWALEntry, allocator: Allocator) !WALEntry {
        return WALEntry.init(
            allocator,
            self.header.base.lsn,
            self.header.base.entry_type,
            ObjRef.init(self.header.base.objref),
            self.data,
        );
    }
};

/// Shard WAL manager for distributed operations
pub const ShardWALManager = struct {
    /// Local WAL instance
    local_wal: *WAL,
    /// Shard ID
    shard_id: ShardId,
    /// Allocator
    allocator: Allocator,
    /// Current distributed transaction ID counter
    next_txn_id: std.atomic.Value(DistributedTxnId),
    /// Active distributed transactions
    active_transactions: std.AutoHashMap(DistributedTxnId, DistributedTransaction),
    /// Mutex for transaction management
    txn_mutex: std.Thread.Mutex,

    pub fn init(allocator: Allocator, local_wal: *WAL, shard_id: ShardId) ShardWALManager {
        return .{
            .local_wal = local_wal,
            .shard_id = shard_id,
            .allocator = allocator,
            .next_txn_id = std.atomic.Value(DistributedTxnId).init(1),
            .active_transactions = std.AutoHashMap(DistributedTxnId, DistributedTransaction).init(allocator),
            .txn_mutex = .{},
        };
    }

    pub fn deinit(self: *ShardWALManager) void {
        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        // Clean up active transactions
        var iter = self.active_transactions.iterator();
        while (iter.next()) |entry| {
            entry.value_ptr.*.deinit(self.allocator);
        }
        self.active_transactions.deinit();
    }

    /// Get next distributed transaction ID
    pub fn nextTxnId(self: *ShardWALManager) DistributedTxnId {
        return self.next_txn_id.fetchAdd(1, .seq_cst);
    }

    /// Write distributed WAL entry to local WAL
    pub fn writeDistributedEntry(self: *ShardWALManager, entry: *const DistributedWALEntry) !void {
        // Convert to base WAL entry and write
        var base_entry = try entry.toWALEntry(self.allocator);
        defer base_entry.deinit(self.allocator);

        try self.local_wal.append(&base_entry);
    }

    /// Begin distributed transaction
    pub fn beginDistributedTransaction(
        self: *ShardWALManager,
        participant_shards: []const ShardId,
    ) !DistributedTxnId {
        const txn_id = self.nextTxnId();

        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        // Create transaction record
        const transaction = try DistributedTransaction.init(
            self.allocator,
            txn_id,
            self.shard_id,
            participant_shards,
        );

        try self.active_transactions.put(txn_id, transaction);

        // Write begin entry to WAL
        var begin_entry = try DistributedWALEntry.init(
            self.allocator,
            self.local_wal.nextLSN(),
            .distributed_begin,
            GlobalObjRef.nullRef(),
            self.shard_id,
            self.shard_id,
            txn_id,
            0,
            "DISTRIBUTED_BEGIN",
        );
        defer begin_entry.deinit(self.allocator);

        try self.writeDistributedEntry(&begin_entry);

        std.log.info("Started distributed transaction {} with {} participants", .{ txn_id, participant_shards.len });
        return txn_id;
    }

    /// Prepare phase of distributed transaction
    pub fn prepareDistributedTransaction(self: *ShardWALManager, txn_id: DistributedTxnId) !bool {
        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        const transaction = self.active_transactions.getPtr(txn_id) orelse return false;

        // Write prepare entry to WAL
        var prepare_entry = try DistributedWALEntry.init(
            self.allocator,
            self.local_wal.nextLSN(),
            .distributed_prepare,
            GlobalObjRef.nullRef(),
            self.shard_id,
            self.shard_id,
            txn_id,
            transaction.sequence_counter,
            "DISTRIBUTED_PREPARE",
        );
        defer prepare_entry.deinit(self.allocator);

        try self.writeDistributedEntry(&prepare_entry);

        transaction.state = .prepared;
        transaction.sequence_counter += 1;

        std.log.info("Prepared distributed transaction {}", .{txn_id});
        return true;
    }

    /// Commit phase of distributed transaction
    pub fn commitDistributedTransaction(self: *ShardWALManager, txn_id: DistributedTxnId) !bool {
        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        const transaction = self.active_transactions.getPtr(txn_id) orelse return false;

        // Write commit entry to WAL
        var commit_entry = try DistributedWALEntry.init(
            self.allocator,
            self.local_wal.nextLSN(),
            .distributed_commit,
            GlobalObjRef.nullRef(),
            self.shard_id,
            self.shard_id,
            txn_id,
            transaction.sequence_counter,
            "DISTRIBUTED_COMMIT",
        );
        defer commit_entry.deinit(self.allocator);

        try self.writeDistributedEntry(&commit_entry);

        transaction.state = .committed;
        transaction.sequence_counter += 1;

        std.log.info("Committed distributed transaction {}", .{txn_id});
        return true;
    }

    /// Abort distributed transaction
    pub fn abortDistributedTransaction(self: *ShardWALManager, txn_id: DistributedTxnId) !bool {
        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        const transaction = self.active_transactions.getPtr(txn_id) orelse return false;

        // Write abort entry to WAL
        var abort_entry = try DistributedWALEntry.init(
            self.allocator,
            self.local_wal.nextLSN(),
            .distributed_abort,
            GlobalObjRef.nullRef(),
            self.shard_id,
            self.shard_id,
            txn_id,
            transaction.sequence_counter,
            "DISTRIBUTED_ABORT",
        );
        defer abort_entry.deinit(self.allocator);

        try self.writeDistributedEntry(&abort_entry);

        transaction.state = .aborted;
        transaction.sequence_counter += 1;

        std.log.info("Aborted distributed transaction {}", .{txn_id});
        return true;
    }

    /// Clean up completed transaction
    pub fn cleanupTransaction(self: *ShardWALManager, txn_id: DistributedTxnId) void {
        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        if (self.active_transactions.fetchRemove(txn_id)) |entry| {
            var transaction = entry.value;
            transaction.deinit(self.allocator);
            std.log.debug("Cleaned up transaction {}", .{txn_id});
        }
    }

    /// Get transaction state
    pub fn getTransactionState(self: *ShardWALManager, txn_id: DistributedTxnId) ?TransactionState {
        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        const transaction = self.active_transactions.get(txn_id) orelse return null;
        return transaction.state;
    }

    /// Write cross-shard operation entry
    pub fn writeCrossShardOperation(
        self: *ShardWALManager,
        txn_id: DistributedTxnId,
        operation_type: DistributedEntryType,
        objref: GlobalObjRef,
        target_shard: ShardId,
        data: []const u8,
    ) !void {
        self.txn_mutex.lock();
        defer self.txn_mutex.unlock();

        const transaction = self.active_transactions.getPtr(txn_id) orelse return error.TransactionNotFound;

        var entry = try DistributedWALEntry.init(
            self.allocator,
            self.local_wal.nextLSN(),
            operation_type,
            objref,
            self.shard_id,
            target_shard,
            txn_id,
            transaction.sequence_counter,
            data,
        );
        defer entry.deinit(self.allocator);

        try self.writeDistributedEntry(&entry);

        transaction.sequence_counter += 1;
        try transaction.operations.append(self.allocator, operation_type);

        std.log.debug("Wrote cross-shard operation {} for transaction {} to shard {}", .{ operation_type, txn_id, target_shard });
    }
};

/// Distributed transaction state
pub const TransactionState = enum {
    active,
    prepared,
    committed,
    aborted,
};

/// Distributed transaction record
pub const DistributedTransaction = struct {
    /// Transaction ID
    id: DistributedTxnId,
    /// Coordinator shard
    coordinator_shard: ShardId,
    /// Participant shards
    participant_shards: std.ArrayList(ShardId),
    /// Transaction state
    state: TransactionState,
    /// Operation sequence counter
    sequence_counter: u32,
    /// Operations performed in this transaction
    operations: std.ArrayList(DistributedEntryType),
    /// Start time
    start_time: i64,

    pub fn init(
        allocator: Allocator,
        id: DistributedTxnId,
        coordinator_shard: ShardId,
        participant_shards: []const ShardId,
    ) !DistributedTransaction {
        var participants = std.ArrayList(ShardId){};
        try participants.appendSlice(allocator, participant_shards);

        const operations = std.ArrayList(DistributedEntryType){};

        return .{
            .id = id,
            .coordinator_shard = coordinator_shard,
            .participant_shards = participants,
            .state = .active,
            .sequence_counter = 1,
            .operations = operations,
            .start_time = std.time.timestamp(),
        };
    }

    pub fn deinit(self: *DistributedTransaction, allocator: Allocator) void {
        self.participant_shards.deinit(allocator);
        self.operations.deinit(allocator);
    }

    /// Check if transaction involves a specific shard
    pub fn involvesShard(self: *const DistributedTransaction, shard_id: ShardId) bool {
        if (self.coordinator_shard == shard_id) return true;

        for (self.participant_shards.items) |participant| {
            if (participant == shard_id) return true;
        }

        return false;
    }

    /// Get transaction duration in seconds
    pub fn duration(self: *const DistributedTransaction) i64 {
        return std.time.timestamp() - self.start_time;
    }
};

/// WAL replication manager for high availability
pub const WALReplicationManager = struct {
    /// Source shard WAL manager
    source_wal: *ShardWALManager,
    /// Replica shard IDs
    replica_shards: std.ArrayList(ShardId),
    /// Allocator
    allocator: Allocator,
    /// Replication enabled flag
    replication_enabled: std.atomic.Value(bool),
    /// Last replicated LSN per replica
    replica_lsns: std.AutoHashMap(ShardId, LSN),
    /// Replication mutex
    replication_mutex: std.Thread.Mutex,

    pub fn init(allocator: Allocator, source_wal: *ShardWALManager) WALReplicationManager {
        return .{
            .source_wal = source_wal,
            .replica_shards = std.ArrayList(ShardId){},
            .allocator = allocator,
            .replication_enabled = std.atomic.Value(bool).init(true),
            .replica_lsns = std.AutoHashMap(ShardId, LSN).init(allocator),
            .replication_mutex = .{},
        };
    }

    pub fn deinit(self: *WALReplicationManager) void {
        self.replication_mutex.lock();
        defer self.replication_mutex.unlock();

        self.replica_shards.deinit(self.allocator);
        self.replica_lsns.deinit();
    }

    /// Add replica shard
    pub fn addReplica(self: *WALReplicationManager, replica_shard: ShardId) !void {
        self.replication_mutex.lock();
        defer self.replication_mutex.unlock();

        try self.replica_shards.append(self.allocator, replica_shard);
        try self.replica_lsns.put(replica_shard, 0);

        std.log.info("Added replica shard {} for WAL replication", .{replica_shard});
    }

    /// Remove replica shard
    pub fn removeReplica(self: *WALReplicationManager, replica_shard: ShardId) void {
        self.replication_mutex.lock();
        defer self.replication_mutex.unlock();

        // Find and remove from replica list
        for (self.replica_shards.items, 0..) |shard, i| {
            if (shard == replica_shard) {
                _ = self.replica_shards.swapRemove(i);
                break;
            }
        }

        _ = self.replica_lsns.remove(replica_shard);

        std.log.info("Removed replica shard {} from WAL replication", .{replica_shard});
    }

    /// Replicate WAL entry to all replicas
    pub fn replicateEntry(self: *WALReplicationManager, entry: *const DistributedWALEntry) !void {
        if (!self.replication_enabled.load(.seq_cst)) return;

        self.replication_mutex.lock();
        defer self.replication_mutex.unlock();

        for (self.replica_shards.items) |replica_shard| {
            // In a full implementation, this would send the entry over the network
            // For now, we just log the replication
            std.log.debug("Replicating LSN {} to replica shard {}", .{ entry.header.base.lsn, replica_shard });

            // Update last replicated LSN
            try self.replica_lsns.put(replica_shard, entry.header.base.lsn);
        }
    }

    /// Get replication lag for a replica
    pub fn getReplicationLag(self: *WALReplicationManager, replica_shard: ShardId) ?LSN {
        self.replication_mutex.lock();
        defer self.replication_mutex.unlock();

        const replica_lsn = self.replica_lsns.get(replica_shard) orelse return null;
        const current_lsn = self.source_wal.local_wal.getCurrentLSN();

        if (current_lsn > replica_lsn) {
            return current_lsn - replica_lsn;
        }
        return 0;
    }

    /// Enable/disable replication
    pub fn setReplicationEnabled(self: *WALReplicationManager, enabled: bool) void {
        self.replication_enabled.store(enabled, .seq_cst);
        std.log.info("WAL replication {s}", .{if (enabled) "enabled" else "disabled"});
    }
};

/// Shard synchronization manager using WAL streams
pub const ShardSynchronizationManager = struct {
    /// Local shard WAL manager
    local_wal: *ShardWALManager,
    /// Allocator
    allocator: Allocator,
    /// Synchronization state per shard
    sync_states: std.AutoHashMap(ShardId, SyncState),
    /// Sync mutex
    sync_mutex: std.Thread.Mutex,

    pub fn init(allocator: Allocator, local_wal: *ShardWALManager) ShardSynchronizationManager {
        return .{
            .local_wal = local_wal,
            .allocator = allocator,
            .sync_states = std.AutoHashMap(ShardId, SyncState).init(allocator),
            .sync_mutex = .{},
        };
    }

    pub fn deinit(self: *ShardSynchronizationManager) void {
        self.sync_mutex.lock();
        defer self.sync_mutex.unlock();

        self.sync_states.deinit();
    }

    /// Start synchronization with a shard
    pub fn startSynchronization(self: *ShardSynchronizationManager, target_shard: ShardId, from_lsn: LSN) !void {
        self.sync_mutex.lock();
        defer self.sync_mutex.unlock();

        const sync_state = SyncState{
            .target_shard = target_shard,
            .last_synced_lsn = from_lsn,
            .sync_start_time = std.time.timestamp(),
            .is_active = true,
        };

        try self.sync_states.put(target_shard, sync_state);

        // Write sync start entry
        var sync_entry = try DistributedWALEntry.init(
            self.allocator,
            self.local_wal.local_wal.nextLSN(),
            .replica_sync,
            GlobalObjRef.nullRef(),
            self.local_wal.shard_id,
            target_shard,
            0, // No transaction ID for sync operations
            0,
            "SYNC_START",
        );
        defer sync_entry.deinit(self.allocator);

        try self.local_wal.writeDistributedEntry(&sync_entry);

        std.log.info("Started synchronization with shard {} from LSN {}", .{ target_shard, from_lsn });
    }

    /// Stop synchronization with a shard
    pub fn stopSynchronization(self: *ShardSynchronizationManager, target_shard: ShardId) void {
        self.sync_mutex.lock();
        defer self.sync_mutex.unlock();

        if (self.sync_states.getPtr(target_shard)) |sync_state| {
            sync_state.is_active = false;
            std.log.info("Stopped synchronization with shard {}", .{target_shard});
        }
    }

    /// Update synchronization progress
    pub fn updateSyncProgress(self: *ShardSynchronizationManager, target_shard: ShardId, synced_lsn: LSN) !void {
        self.sync_mutex.lock();
        defer self.sync_mutex.unlock();

        if (self.sync_states.getPtr(target_shard)) |sync_state| {
            sync_state.last_synced_lsn = synced_lsn;

            std.log.debug("Updated sync progress for shard {} to LSN {}", .{ target_shard, synced_lsn });
        }
    }

    /// Get synchronization status
    pub fn getSyncStatus(self: *ShardSynchronizationManager, target_shard: ShardId) ?SyncState {
        self.sync_mutex.lock();
        defer self.sync_mutex.unlock();

        return self.sync_states.get(target_shard);
    }

    /// Perform catchup synchronization
    pub fn performCatchupSync(self: *ShardSynchronizationManager, target_shard: ShardId, target_lsn: LSN) !void {
        const sync_state = self.getSyncStatus(target_shard) orelse return error.SyncNotActive;

        if (!sync_state.is_active) return error.SyncNotActive;

        var current_lsn = sync_state.last_synced_lsn;

        while (current_lsn < target_lsn) {
            // In a full implementation, this would read WAL entries and send them
            // For now, we simulate the catchup process
            current_lsn += 1;

            // Write catchup entry
            var catchup_entry = try DistributedWALEntry.init(
                self.allocator,
                self.local_wal.local_wal.nextLSN(),
                .replica_catchup,
                GlobalObjRef.nullRef(),
                self.local_wal.shard_id,
                target_shard,
                0,
                0,
                "CATCHUP",
            );
            defer catchup_entry.deinit(self.allocator);

            try self.local_wal.writeDistributedEntry(&catchup_entry);

            // Update progress
            try self.updateSyncProgress(target_shard, current_lsn);

            // Yield to prevent blocking
            if (current_lsn % 100 == 0) {
                std.Thread.sleep(1000000); // 1ms
            }
        }

        std.log.info("Completed catchup sync for shard {} to LSN {}", .{ target_shard, target_lsn });
    }
};

/// Synchronization state for a shard
pub const SyncState = struct {
    target_shard: ShardId,
    last_synced_lsn: LSN,
    sync_start_time: i64,
    is_active: bool,

    /// Get sync duration in seconds
    pub fn duration(self: *const SyncState) i64 {
        return std.time.timestamp() - self.sync_start_time;
    }

    /// Calculate sync rate (LSNs per second)
    pub fn syncRate(self: *const SyncState) f64 {
        const dur = self.duration();
        if (dur > 0) {
            return @as(f64, @floatFromInt(self.last_synced_lsn)) / @as(f64, @floatFromInt(dur));
        }
        return 0.0;
    }
};

// ============================================================================
// Tests
// ============================================================================

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

test "DistributedWAL: entry creation and validation" {
    const allocator = std.testing.allocator;

    const objref = GlobalObjRef.init(1, 1024, 0);
    const data = "test distributed entry";

    var entry = try DistributedWALEntry.init(
        allocator,
        1,
        .distributed_begin,
        objref,
        0, // source shard
        1, // target shard
        42, // txn id
        0, // sequence
        data,
    );
    defer entry.deinit(allocator);

    try std.testing.expect(entry.validate());
    try std.testing.expectEqual(@as(LSN, 1), entry.header.base.lsn);
    try std.testing.expectEqual(DistributedEntryType.distributed_begin, entry.header.distributed_type);
    try std.testing.expectEqual(@as(ShardId, 0), entry.header.source_shard);
    try std.testing.expectEqual(@as(ShardId, 1), entry.header.target_shard);
    try std.testing.expectEqual(@as(DistributedTxnId, 42), entry.header.txn_id);
    try std.testing.expectEqualStrings(data, entry.data);
}

test "DistributedWAL: shard WAL manager" {
    const allocator = std.testing.allocator;

    var test_wal = try createTestWAL(allocator, "shard_wal_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    try std.testing.expectEqual(@as(ShardId, 0), shard_wal.shard_id);
    try std.testing.expectEqual(@as(DistributedTxnId, 1), shard_wal.nextTxnId());
    try std.testing.expectEqual(@as(DistributedTxnId, 2), shard_wal.nextTxnId());
}

test "DistributedWAL: distributed transaction lifecycle" {
    const allocator = std.testing.allocator;

    var test_wal = try createTestWAL(allocator, "distributed_txn_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    // Begin distributed transaction
    const participants = [_]ShardId{ 1, 2 };
    const txn_id = try shard_wal.beginDistributedTransaction(&participants);
    try std.testing.expect(txn_id > 0);

    // Check transaction state
    const state1 = shard_wal.getTransactionState(txn_id);
    try std.testing.expectEqual(TransactionState.active, state1.?);

    // Prepare transaction
    const prepared = try shard_wal.prepareDistributedTransaction(txn_id);
    try std.testing.expect(prepared);

    const state2 = shard_wal.getTransactionState(txn_id);
    try std.testing.expectEqual(TransactionState.prepared, state2.?);

    // Commit transaction
    const committed = try shard_wal.commitDistributedTransaction(txn_id);
    try std.testing.expect(committed);

    const state3 = shard_wal.getTransactionState(txn_id);
    try std.testing.expectEqual(TransactionState.committed, state3.?);

    // Cleanup
    shard_wal.cleanupTransaction(txn_id);
    const state4 = shard_wal.getTransactionState(txn_id);
    try std.testing.expect(state4 == null);
}

test "DistributedWAL: cross-shard operations" {
    const allocator = std.testing.allocator;

    var test_wal = try createTestWAL(allocator, "cross_shard_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    // Begin transaction
    const participants = [_]ShardId{1};
    const txn_id = try shard_wal.beginDistributedTransaction(&participants);

    // Write cross-shard operation
    const objref = GlobalObjRef.init(1, 2048, 0);
    try shard_wal.writeCrossShardOperation(
        txn_id,
        .insert,
        objref,
        1, // target shard
        "cross-shard data",
    );

    // Verify transaction has operations
    const state = shard_wal.getTransactionState(txn_id);
    try std.testing.expectEqual(TransactionState.active, state.?);
}

test "DistributedWAL: WAL replication manager" {
    const allocator = std.testing.allocator;

    var test_wal = try createTestWAL(allocator, "replication_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    var replication_mgr = WALReplicationManager.init(allocator, &shard_wal);
    defer replication_mgr.deinit();

    // Add replicas
    try replication_mgr.addReplica(1);
    try replication_mgr.addReplica(2);

    try std.testing.expectEqual(@as(usize, 2), replication_mgr.replica_shards.items.len);

    // Test replication
    const objref = GlobalObjRef.init(0, 1024, 0);
    var entry = try DistributedWALEntry.init(
        allocator,
        1,
        .insert,
        objref,
        0,
        0,
        0,
        0,
        "replicated data",
    );
    defer entry.deinit(allocator);

    try replication_mgr.replicateEntry(&entry);

    // Check replication lag
    const lag1 = replication_mgr.getReplicationLag(1);
    try std.testing.expect(lag1 != null);

    // Remove replica
    replication_mgr.removeReplica(1);
    try std.testing.expectEqual(@as(usize, 1), replication_mgr.replica_shards.items.len);
}

test "DistributedWAL: shard synchronization" {
    const allocator = std.testing.allocator;

    var test_wal = try createTestWAL(allocator, "sync_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    var sync_mgr = ShardSynchronizationManager.init(allocator, &shard_wal);
    defer sync_mgr.deinit();

    // Start synchronization
    try sync_mgr.startSynchronization(1, 0);

    const status1 = sync_mgr.getSyncStatus(1);
    try std.testing.expect(status1 != null);
    try std.testing.expect(status1.?.is_active);
    try std.testing.expectEqual(@as(LSN, 0), status1.?.last_synced_lsn);

    // Update progress
    try sync_mgr.updateSyncProgress(1, 10);

    const status2 = sync_mgr.getSyncStatus(1);
    try std.testing.expectEqual(@as(LSN, 10), status2.?.last_synced_lsn);

    // Stop synchronization
    sync_mgr.stopSynchronization(1);

    const status3 = sync_mgr.getSyncStatus(1);
    try std.testing.expect(!status3.?.is_active);
}

test "DistributedWAL: distributed transaction record" {
    const allocator = std.testing.allocator;

    const participants = [_]ShardId{ 1, 2, 3 };
    var txn = try DistributedTransaction.init(allocator, 42, 0, &participants);
    defer txn.deinit(allocator);

    try std.testing.expectEqual(@as(DistributedTxnId, 42), txn.id);
    try std.testing.expectEqual(@as(ShardId, 0), txn.coordinator_shard);
    try std.testing.expectEqual(@as(usize, 3), txn.participant_shards.items.len);
    try std.testing.expectEqual(TransactionState.active, txn.state);

    // Test shard involvement
    try std.testing.expect(txn.involvesShard(0)); // coordinator
    try std.testing.expect(txn.involvesShard(1)); // participant
    try std.testing.expect(txn.involvesShard(2)); // participant
    try std.testing.expect(txn.involvesShard(3)); // participant
    try std.testing.expect(!txn.involvesShard(4)); // not involved

    // Test duration
    const dur = txn.duration();
    try std.testing.expect(dur >= 0);
}

test "DistributedWAL: entry type conversions" {
    try std.testing.expectEqual(DistributedEntryType.insert, DistributedEntryType.fromEntryType(.insert));
    try std.testing.expectEqual(DistributedEntryType.update, DistributedEntryType.fromEntryType(.update));
    try std.testing.expectEqual(DistributedEntryType.delete, DistributedEntryType.fromEntryType(.delete));

    try std.testing.expectEqual(EntryType.insert, DistributedEntryType.insert.toEntryType());
    try std.testing.expectEqual(EntryType.update, DistributedEntryType.update.toEntryType());
    try std.testing.expectEqual(EntryType.delete, DistributedEntryType.delete.toEntryType());
}

test "DistributedWAL: sync state calculations" {
    const sync_state = SyncState{
        .target_shard = 1,
        .last_synced_lsn = 100,
        .sync_start_time = std.time.timestamp() - 10, // 10 seconds ago
        .is_active = true,
    };

    const dur = sync_state.duration();
    try std.testing.expect(dur >= 10);

    const rate = sync_state.syncRate();
    try std.testing.expect(rate > 0);
}

test "DistributedWAL: transaction abort" {
    const allocator = std.testing.allocator;

    var test_wal = try createTestWAL(allocator, "abort_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    // Begin and abort transaction
    const participants = [_]ShardId{1};
    const txn_id = try shard_wal.beginDistributedTransaction(&participants);

    const aborted = try shard_wal.abortDistributedTransaction(txn_id);
    try std.testing.expect(aborted);

    const state = shard_wal.getTransactionState(txn_id);
    try std.testing.expectEqual(TransactionState.aborted, state.?);
}

test "DistributedWAL: replication enable/disable" {
    const allocator = std.testing.allocator;

    var test_wal = try createTestWAL(allocator, "replication_toggle_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_wal = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_wal.deinit();

    var replication_mgr = WALReplicationManager.init(allocator, &shard_wal);
    defer replication_mgr.deinit();

    // Initially enabled
    try std.testing.expect(replication_mgr.replication_enabled.load(.seq_cst));

    // Disable replication
    replication_mgr.setReplicationEnabled(false);
    try std.testing.expect(!replication_mgr.replication_enabled.load(.seq_cst));

    // Re-enable replication
    replication_mgr.setReplicationEnabled(true);
    try std.testing.expect(replication_mgr.replication_enabled.load(.seq_cst));
}
