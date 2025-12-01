//! Comprehensive tests for Distributed WAL Implementation
//!
//! This test file verifies all aspects of the distributed WAL functionality
//! including shard-specific WAL management, cross-shard coordination,
//! replication, and synchronization.

const std = @import("std");
const testing = std.testing;
const Allocator = std.mem.Allocator;

const WAL = @import("wal.zig").WAL;
const WALEntry = @import("wal.zig").WALEntry;
const EntryType = @import("wal.zig").EntryType;
const LSN = @import("wal.zig").LSN;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const ObjRef = @import("objref.zig").ObjRef;

const DistributedWAL = @import("distributed_wal.zig");
const DistributedWALEntry = DistributedWAL.DistributedWALEntry;
const DistributedWALEntryHeader = DistributedWAL.DistributedWALEntryHeader;
const DistributedEntryType = DistributedWAL.DistributedEntryType;
const DistributedTxnId = DistributedWAL.DistributedTxnId;
const ShardWALManager = DistributedWAL.ShardWALManager;
const DistributedTransaction = DistributedWAL.DistributedTransaction;
const TransactionState = DistributedWAL.TransactionState;
const WALReplicationManager = DistributedWAL.WALReplicationManager;
const ShardSynchronizationManager = DistributedWAL.ShardSynchronizationManager;
const SyncState = DistributedWAL.SyncState;

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

test "DistributedWAL: comprehensive distributed transaction test" {
    const allocator = testing.allocator;

    // Create WAL for coordinator shard
    var coordinator_wal = try createTestWAL(allocator, "coordinator_wal");
    defer coordinator_wal.dir.cleanup();
    defer coordinator_wal.wal.deinit();

    // Create WAL for participant shard
    var participant_wal = try createTestWAL(allocator, "participant_wal");
    defer participant_wal.dir.cleanup();
    defer participant_wal.wal.deinit();

    // Create shard WAL managers
    var coordinator_mgr = ShardWALManager.init(allocator, &coordinator_wal.wal, 0);
    defer coordinator_mgr.deinit();

    var participant_mgr = ShardWALManager.init(allocator, &participant_wal.wal, 1);
    defer participant_mgr.deinit();

    // Begin distributed transaction on coordinator
    const participants = [_]ShardId{1};
    const txn_id = try coordinator_mgr.beginDistributedTransaction(&participants);

    // Simulate cross-shard operations
    const objref1 = GlobalObjRef.init(0, 1024, 0);
    try coordinator_mgr.writeCrossShardOperation(
        txn_id,
        .insert,
        objref1,
        1, // target shard
        "data for shard 1",
    );

    const objref2 = GlobalObjRef.init(1, 2048, 0);
    try coordinator_mgr.writeCrossShardOperation(
        txn_id,
        .update,
        objref2,
        1, // target shard
        "updated data for shard 1",
    );

    // Prepare phase on both shards
    const coord_prepared = try coordinator_mgr.prepareDistributedTransaction(txn_id);
    try testing.expect(coord_prepared);

    // Simulate participant preparation (in real system, this would be over network)
    const part_txn_id = try participant_mgr.beginDistributedTransaction(&[_]ShardId{0});
    const part_prepared = try participant_mgr.prepareDistributedTransaction(part_txn_id);
    try testing.expect(part_prepared);

    // Commit phase on both shards
    const coord_committed = try coordinator_mgr.commitDistributedTransaction(txn_id);
    try testing.expect(coord_committed);

    const part_committed = try participant_mgr.commitDistributedTransaction(part_txn_id);
    try testing.expect(part_committed);

    // Verify final states
    try testing.expectEqual(TransactionState.committed, coordinator_mgr.getTransactionState(txn_id).?);
    try testing.expectEqual(TransactionState.committed, participant_mgr.getTransactionState(part_txn_id).?);

    // Cleanup transactions
    coordinator_mgr.cleanupTransaction(txn_id);
    participant_mgr.cleanupTransaction(part_txn_id);

    try testing.expect(coordinator_mgr.getTransactionState(txn_id) == null);
    try testing.expect(participant_mgr.getTransactionState(part_txn_id) == null);
}

test "DistributedWAL: WAL replication with multiple replicas" {
    const allocator = testing.allocator;

    // Create primary WAL
    var primary_wal = try createTestWAL(allocator, "primary_replication_wal");
    defer primary_wal.dir.cleanup();
    defer primary_wal.wal.deinit();

    var primary_mgr = ShardWALManager.init(allocator, &primary_wal.wal, 0);
    defer primary_mgr.deinit();

    // Create replication manager
    var replication_mgr = WALReplicationManager.init(allocator, &primary_mgr);
    defer replication_mgr.deinit();

    // Add multiple replicas
    try replication_mgr.addReplica(1);
    try replication_mgr.addReplica(2);
    try replication_mgr.addReplica(3);

    try testing.expectEqual(@as(usize, 3), replication_mgr.replica_shards.items.len);

    // Create and replicate multiple entries
    for (0..10) |i| {
        const objref = GlobalObjRef.init(0, 1024 + i * 64, 0);
        const data = try std.fmt.allocPrint(allocator, "replicated entry {}", .{i});
        defer allocator.free(data);

        var entry = try DistributedWALEntry.init(
            allocator,
            @intCast(i + 1),
            .insert,
            objref,
            0, // source shard
            0, // target shard
            0, // no transaction
            0,
            data,
        );
        defer entry.deinit(allocator);

        try replication_mgr.replicateEntry(&entry);
    }

    // Check replication lag for all replicas
    for (1..4) |shard_id| {
        const lag = replication_mgr.getReplicationLag(@intCast(shard_id));
        try testing.expect(lag != null);
        try testing.expect(lag.? >= 0);
    }

    // Test disabling replication
    replication_mgr.setReplicationEnabled(false);
    try testing.expect(!replication_mgr.replication_enabled.load(.seq_cst));

    // Create entry while replication is disabled
    const objref = GlobalObjRef.init(0, 2048, 0);
    var disabled_entry = try DistributedWALEntry.init(
        allocator,
        11,
        .insert,
        objref,
        0,
        0,
        0,
        0,
        "disabled replication entry",
    );
    defer disabled_entry.deinit(allocator);

    // This should not replicate
    try replication_mgr.replicateEntry(&disabled_entry);

    // Re-enable replication
    replication_mgr.setReplicationEnabled(true);
    try testing.expect(replication_mgr.replication_enabled.load(.seq_cst));

    // Remove some replicas
    replication_mgr.removeReplica(2);
    try testing.expectEqual(@as(usize, 2), replication_mgr.replica_shards.items.len);

    replication_mgr.removeReplica(3);
    try testing.expectEqual(@as(usize, 1), replication_mgr.replica_shards.items.len);
}

test "DistributedWAL: shard synchronization with catchup" {
    const allocator = testing.allocator;

    // Create source WAL
    var source_wal = try createTestWAL(allocator, "sync_source_wal");
    defer source_wal.dir.cleanup();
    defer source_wal.wal.deinit();

    var source_mgr = ShardWALManager.init(allocator, &source_wal.wal, 0);
    defer source_mgr.deinit();

    // Create synchronization manager
    var sync_mgr = ShardSynchronizationManager.init(allocator, &source_mgr);
    defer sync_mgr.deinit();

    // Start synchronization with multiple target shards
    try sync_mgr.startSynchronization(1, 0);
    try sync_mgr.startSynchronization(2, 5);
    try sync_mgr.startSynchronization(3, 10);

    // Verify initial sync states
    const status1 = sync_mgr.getSyncStatus(1);
    try testing.expect(status1 != null);
    try testing.expect(status1.?.is_active);
    try testing.expectEqual(@as(LSN, 0), status1.?.last_synced_lsn);

    const status2 = sync_mgr.getSyncStatus(2);
    try testing.expect(status2 != null);
    try testing.expectEqual(@as(LSN, 5), status2.?.last_synced_lsn);

    const status3 = sync_mgr.getSyncStatus(3);
    try testing.expect(status3 != null);
    try testing.expectEqual(@as(LSN, 10), status3.?.last_synced_lsn);

    // Simulate progress updates
    try sync_mgr.updateSyncProgress(1, 15);
    try sync_mgr.updateSyncProgress(2, 20);
    try sync_mgr.updateSyncProgress(3, 25);

    // Verify updated progress
    const updated_status1 = sync_mgr.getSyncStatus(1);
    try testing.expectEqual(@as(LSN, 15), updated_status1.?.last_synced_lsn);

    // Perform catchup synchronization
    try sync_mgr.performCatchupSync(1, 50);

    const catchup_status = sync_mgr.getSyncStatus(1);
    try testing.expectEqual(@as(LSN, 50), catchup_status.?.last_synced_lsn);

    // Test sync rate calculation
    const rate = catchup_status.?.syncRate();
    try testing.expect(rate >= 0);

    // Stop synchronization for one shard
    sync_mgr.stopSynchronization(2);
    const stopped_status = sync_mgr.getSyncStatus(2);
    try testing.expect(!stopped_status.?.is_active);

    // Verify other shards still active
    const still_active = sync_mgr.getSyncStatus(3);
    try testing.expect(still_active.?.is_active);
}

test "DistributedWAL: transaction abort and rollback" {
    const allocator = testing.allocator;

    var test_wal = try createTestWAL(allocator, "abort_rollback_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_mgr = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_mgr.deinit();

    // Begin transaction with multiple participants
    const participants = [_]ShardId{ 1, 2, 3 };
    const txn_id = try shard_mgr.beginDistributedTransaction(&participants);

    // Perform several operations
    const objref1 = GlobalObjRef.init(1, 1024, 0);
    try shard_mgr.writeCrossShardOperation(txn_id, .insert, objref1, 1, "operation 1");

    const objref2 = GlobalObjRef.init(2, 2048, 0);
    try shard_mgr.writeCrossShardOperation(txn_id, .update, objref2, 2, "operation 2");

    const objref3 = GlobalObjRef.init(3, 3072, 0);
    try shard_mgr.writeCrossShardOperation(txn_id, .delete, objref3, 3, "operation 3");

    // Verify transaction is active with operations
    const active_state = shard_mgr.getTransactionState(txn_id);
    try testing.expectEqual(TransactionState.active, active_state.?);

    // Prepare transaction
    const prepared = try shard_mgr.prepareDistributedTransaction(txn_id);
    try testing.expect(prepared);

    const prepared_state = shard_mgr.getTransactionState(txn_id);
    try testing.expectEqual(TransactionState.prepared, prepared_state.?);

    // Abort the transaction instead of committing
    const aborted = try shard_mgr.abortDistributedTransaction(txn_id);
    try testing.expect(aborted);

    const aborted_state = shard_mgr.getTransactionState(txn_id);
    try testing.expectEqual(TransactionState.aborted, aborted_state.?);

    // Cleanup
    shard_mgr.cleanupTransaction(txn_id);
    try testing.expect(shard_mgr.getTransactionState(txn_id) == null);
}

test "DistributedWAL: concurrent transaction management" {
    const allocator = testing.allocator;

    var test_wal = try createTestWAL(allocator, "concurrent_txn_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_mgr = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_mgr.deinit();

    // Create multiple concurrent transactions
    const num_transactions = 5;
    var transaction_ids: [num_transactions]DistributedTxnId = undefined;

    for (0..num_transactions) |i| {
        const participants = [_]ShardId{@intCast(i + 1)};
        transaction_ids[i] = try shard_mgr.beginDistributedTransaction(&participants);
    }

    // Verify all transactions are active
    for (transaction_ids) |txn_id| {
        const state = shard_mgr.getTransactionState(txn_id);
        try testing.expectEqual(TransactionState.active, state.?);
    }

    // Perform operations on each transaction
    for (transaction_ids, 0..) |txn_id, i| {
        const objref = GlobalObjRef.init(@intCast(i + 1), 1024 + i * 64, 0);
        const data = try std.fmt.allocPrint(allocator, "concurrent operation {}", .{i});
        defer allocator.free(data);

        try shard_mgr.writeCrossShardOperation(
            txn_id,
            .insert,
            objref,
            @intCast(i + 1),
            data,
        );
    }

    // Prepare all transactions
    for (transaction_ids) |txn_id| {
        const prepared = try shard_mgr.prepareDistributedTransaction(txn_id);
        try testing.expect(prepared);
    }

    // Commit half, abort half
    for (transaction_ids, 0..) |txn_id, i| {
        if (i % 2 == 0) {
            const committed = try shard_mgr.commitDistributedTransaction(txn_id);
            try testing.expect(committed);
            try testing.expectEqual(TransactionState.committed, shard_mgr.getTransactionState(txn_id).?);
        } else {
            const aborted = try shard_mgr.abortDistributedTransaction(txn_id);
            try testing.expect(aborted);
            try testing.expectEqual(TransactionState.aborted, shard_mgr.getTransactionState(txn_id).?);
        }
    }

    // Cleanup all transactions
    for (transaction_ids) |txn_id| {
        shard_mgr.cleanupTransaction(txn_id);
        try testing.expect(shard_mgr.getTransactionState(txn_id) == null);
    }
}

test "DistributedWAL: entry header validation and checksums" {
    const allocator = testing.allocator;

    const objref = GlobalObjRef.init(1, 2048, GlobalObjRef.Flags.COMPRESSED_LZ4);
    const data = "checksum validation test data";

    // Create distributed WAL entry
    var entry = try DistributedWALEntry.init(
        allocator,
        42, // LSN
        .distributed_prepare,
        objref,
        0, // source shard
        1, // target shard
        123, // txn id
        5, // sequence
        data,
    );
    defer entry.deinit(allocator);

    // Verify entry is valid
    try testing.expect(entry.validate());

    // Verify header fields
    try testing.expectEqual(@as(LSN, 42), entry.header.base.lsn);
    try testing.expectEqual(DistributedEntryType.distributed_prepare, entry.header.distributed_type);
    try testing.expectEqual(@as(ShardId, 0), entry.header.source_shard);
    try testing.expectEqual(@as(ShardId, 1), entry.header.target_shard);
    try testing.expectEqual(@as(DistributedTxnId, 123), entry.header.txn_id);
    try testing.expectEqual(@as(u32, 5), entry.header.txn_sequence);
    try testing.expectEqualStrings(data, entry.data);

    // Test checksum corruption
    var corrupted_entry = entry;
    corrupted_entry.header.base.checksum = 0xDEADBEEF;
    try testing.expect(!corrupted_entry.validate());

    // Test data corruption
    var corrupted_data = try allocator.dupe(u8, entry.data);
    defer allocator.free(corrupted_data);
    corrupted_data[0] = ~corrupted_data[0]; // Flip bits

    var data_corrupted_entry = entry;
    data_corrupted_entry.data = corrupted_data;
    try testing.expect(!data_corrupted_entry.validate());
}

test "DistributedWAL: transaction record functionality" {
    const allocator = testing.allocator;

    const participants = [_]ShardId{ 1, 2, 3, 4, 5 };
    var txn = try DistributedTransaction.init(allocator, 999, 0, &participants);
    defer txn.deinit(allocator);

    // Test basic properties
    try testing.expectEqual(@as(DistributedTxnId, 999), txn.id);
    try testing.expectEqual(@as(ShardId, 0), txn.coordinator_shard);
    try testing.expectEqual(@as(usize, 5), txn.participant_shards.items.len);
    try testing.expectEqual(TransactionState.active, txn.state);
    try testing.expectEqual(@as(u32, 1), txn.sequence_counter);

    // Test shard involvement
    try testing.expect(txn.involvesShard(0)); // coordinator
    for (1..6) |shard_id| {
        try testing.expect(txn.involvesShard(@intCast(shard_id))); // participants
    }
    try testing.expect(!txn.involvesShard(6)); // not involved
    try testing.expect(!txn.involvesShard(100)); // not involved

    // Test duration calculation
    const initial_duration = txn.duration();
    try testing.expect(initial_duration >= 0);

    // Wait a bit and check duration increased
    std.Thread.sleep(1000000); // 1ms
    const later_duration = txn.duration();
    try testing.expect(later_duration >= initial_duration);

    // Test operations tracking
    try testing.expectEqual(@as(usize, 0), txn.operations.items.len);

    try txn.operations.append(allocator, .insert);
    try txn.operations.append(allocator, .update);
    try txn.operations.append(allocator, .delete);

    try testing.expectEqual(@as(usize, 3), txn.operations.items.len);
    try testing.expectEqual(DistributedEntryType.insert, txn.operations.items[0]);
    try testing.expectEqual(DistributedEntryType.update, txn.operations.items[1]);
    try testing.expectEqual(DistributedEntryType.delete, txn.operations.items[2]);
}

test "DistributedWAL: sync state metrics and calculations" {
    // Test sync state with various scenarios
    const base_time = std.time.timestamp();

    // Fast sync scenario
    const fast_sync = SyncState{
        .target_shard = 1,
        .last_synced_lsn = 1000,
        .sync_start_time = base_time - 10, // 10 seconds ago
        .is_active = true,
    };

    const fast_duration = fast_sync.duration();
    try testing.expect(fast_duration >= 10);

    const fast_rate = fast_sync.syncRate();
    try testing.expect(fast_rate >= 100.0); // 1000 LSNs / 10 seconds = 100 LSNs/sec

    // Slow sync scenario
    const slow_sync = SyncState{
        .target_shard = 2,
        .last_synced_lsn = 50,
        .sync_start_time = base_time - 100, // 100 seconds ago
        .is_active = true,
    };

    const slow_duration = slow_sync.duration();
    try testing.expect(slow_duration >= 100);

    const slow_rate = slow_sync.syncRate();
    try testing.expect(slow_rate <= 1.0); // 50 LSNs / 100 seconds = 0.5 LSNs/sec

    // No progress scenario
    const no_progress_sync = SyncState{
        .target_shard = 3,
        .last_synced_lsn = 0,
        .sync_start_time = base_time - 60,
        .is_active = false,
    };

    const no_progress_rate = no_progress_sync.syncRate();
    try testing.expectEqual(@as(f64, 0.0), no_progress_rate);
}

test "DistributedWAL: entry type conversions and compatibility" {
    // Test all standard entry type conversions
    const standard_types = [_]EntryType{ .insert, .update, .delete, .checkpoint, .commit, .abort };

    for (standard_types) |entry_type| {
        const distributed_type = DistributedEntryType.fromEntryType(entry_type);
        const converted_back = distributed_type.toEntryType();
        try testing.expectEqual(entry_type, converted_back);
    }

    // Test distributed-specific types
    const distributed_types = [_]DistributedEntryType{
        .distributed_begin,
        .distributed_prepare,
        .distributed_commit,
        .distributed_abort,
        .shard_join,
        .shard_leave,
        .shard_rebalance,
        .replica_sync,
        .replica_catchup,
    };

    for (distributed_types) |dist_type| {
        const converted = dist_type.toEntryType();
        // All distributed types should convert to a valid EntryType
        try testing.expect(converted == .commit or converted == .insert or converted == .update or converted == .delete or converted == .checkpoint or converted == .abort);

        // Test string representation
        const type_str = dist_type.toString();
        try testing.expect(type_str.len > 0);
    }
}

test "DistributedWAL: WAL entry conversion compatibility" {
    const allocator = testing.allocator;

    const objref = GlobalObjRef.init(2, 4096, 0);
    const data = "conversion compatibility test";

    // Create distributed WAL entry
    var dist_entry = try DistributedWALEntry.init(
        allocator,
        100,
        .insert,
        objref,
        0, // source shard
        2, // target shard
        456, // txn id
        3, // sequence
        data,
    );
    defer dist_entry.deinit(allocator);

    // Convert to base WAL entry
    var base_entry = try dist_entry.toWALEntry(allocator);
    defer base_entry.deinit(allocator);

    // Verify conversion preserved essential data
    try testing.expectEqual(@as(LSN, 100), base_entry.header.lsn);
    try testing.expectEqual(EntryType.insert, base_entry.header.entry_type);
    try testing.expectEqual(@as(u64, 4096), base_entry.header.objref);
    try testing.expectEqualStrings(data, base_entry.data);

    // Verify base entry is valid
    try testing.expect(base_entry.validate());
}

test "DistributedWAL: large scale transaction simulation" {
    const allocator = testing.allocator;

    var test_wal = try createTestWAL(allocator, "large_scale_test");
    defer test_wal.dir.cleanup();
    defer test_wal.wal.deinit();

    var shard_mgr = ShardWALManager.init(allocator, &test_wal.wal, 0);
    defer shard_mgr.deinit();

    var replication_mgr = WALReplicationManager.init(allocator, &shard_mgr);
    defer replication_mgr.deinit();

    var sync_mgr = ShardSynchronizationManager.init(allocator, &shard_mgr);
    defer sync_mgr.deinit();

    // Add replicas and sync targets
    try replication_mgr.addReplica(1);
    try replication_mgr.addReplica(2);
    try sync_mgr.startSynchronization(3, 0);
    try sync_mgr.startSynchronization(4, 0);

    // Create many concurrent transactions
    const num_transactions = 20;
    var transaction_ids: [num_transactions]DistributedTxnId = undefined;

    for (0..num_transactions) |i| {
        const participants = [_]ShardId{@intCast((i % 3) + 1)};
        transaction_ids[i] = try shard_mgr.beginDistributedTransaction(&participants);
    }

    // Perform many operations per transaction
    for (transaction_ids, 0..) |txn_id, i| {
        for (0..10) |j| {
            const objref = GlobalObjRef.init(@intCast((i % 3) + 1), 1024 + i * 64 + j * 8, 0);
            const data = try std.fmt.allocPrint(allocator, "txn {} op {}", .{ i, j });
            defer allocator.free(data);

            try shard_mgr.writeCrossShardOperation(
                txn_id,
                if (j % 3 == 0) .insert else if (j % 3 == 1) .update else .delete,
                objref,
                @intCast((i % 3) + 1),
                data,
            );

            // Replicate some entries
            if (j % 2 == 0) {
                var entry = try DistributedWALEntry.init(
                    allocator,
                    @intCast(i * 10 + j + 1),
                    .insert,
                    objref,
                    0,
                    @intCast((i % 3) + 1),
                    txn_id,
                    @intCast(j),
                    data,
                );
                defer entry.deinit(allocator);

                try replication_mgr.replicateEntry(&entry);
            }
        }
    }

    // Prepare all transactions
    for (transaction_ids) |txn_id| {
        const prepared = try shard_mgr.prepareDistributedTransaction(txn_id);
        try testing.expect(prepared);
    }

    // Commit most transactions, abort a few
    var committed_count: usize = 0;
    var aborted_count: usize = 0;

    for (transaction_ids, 0..) |txn_id, i| {
        if (i % 7 == 0) { // Abort every 7th transaction
            const aborted = try shard_mgr.abortDistributedTransaction(txn_id);
            try testing.expect(aborted);
            aborted_count += 1;
        } else {
            const committed = try shard_mgr.commitDistributedTransaction(txn_id);
            try testing.expect(committed);
            committed_count += 1;
        }
    }

    try testing.expect(committed_count > 0);
    try testing.expect(aborted_count > 0);
    try testing.expectEqual(num_transactions, committed_count + aborted_count);

    // Update sync progress
    try sync_mgr.updateSyncProgress(3, 100);
    try sync_mgr.updateSyncProgress(4, 150);

    // Verify replication lag
    const lag1 = replication_mgr.getReplicationLag(1);
    const lag2 = replication_mgr.getReplicationLag(2);
    try testing.expect(lag1 != null);
    try testing.expect(lag2 != null);

    // Cleanup all transactions
    for (transaction_ids) |txn_id| {
        shard_mgr.cleanupTransaction(txn_id);
    }

    // Verify all transactions cleaned up
    for (transaction_ids) |txn_id| {
        try testing.expect(shard_mgr.getTransactionState(txn_id) == null);
    }
}
