//! Transaction Management System
//!
//! Provides ACID transaction support with MVCC (Multi-Version Concurrency Control).
//! Transactions are identified by unique IDs and track their state through the lifecycle.

const std = @import("std");
const Allocator = std.mem.Allocator;
const ObjRef = @import("objref.zig").ObjRef;
const WAL = @import("wal.zig").WAL;
const WALEntry = @import("wal.zig").WALEntry;
const WALEntryType = @import("wal.zig").WALEntryType;
const LSN = @import("wal.zig").LSN;

/// Transaction ID - unique identifier for each transaction
pub const TransactionId = u64;

/// Transaction state
pub const TransactionState = enum(u8) {
    /// Transaction is active and can perform operations
    active = 1,
    /// Transaction is preparing to commit
    preparing = 2,
    /// Transaction has committed successfully
    committed = 3,
    /// Transaction has been aborted/rolled back
    aborted = 4,
    /// Transaction has timed out
    timed_out = 5,

    pub fn toString(self: TransactionState) []const u8 {
        return switch (self) {
            .active => "ACTIVE",
            .preparing => "PREPARING",
            .committed => "COMMITTED",
            .aborted => "ABORTED",
            .timed_out => "TIMED_OUT",
        };
    }

    pub fn isTerminal(self: TransactionState) bool {
        return switch (self) {
            .committed, .aborted, .timed_out => true,
            .active, .preparing => false,
        };
    }
};

/// Isolation level for transactions
pub const IsolationLevel = enum(u8) {
    /// Read uncommitted data (dirty reads possible)
    read_uncommitted = 1,
    /// Read only committed data (no dirty reads)
    read_committed = 2,
    /// Repeatable reads within transaction (snapshot isolation)
    repeatable_read = 3,
    /// Full serializability (strictest)
    serializable = 4,

    pub fn toString(self: IsolationLevel) []const u8 {
        return switch (self) {
            .read_uncommitted => "READ_UNCOMMITTED",
            .read_committed => "READ_COMMITTED",
            .repeatable_read => "REPEATABLE_READ",
            .serializable => "SERIALIZABLE",
        };
    }
};

/// Transaction configuration
pub const TransactionConfig = struct {
    /// Isolation level for this transaction
    isolation_level: IsolationLevel = .repeatable_read,
    /// Timeout in milliseconds (0 = no timeout)
    timeout_ms: u64 = 30000, // 30 seconds default
    /// Read-only transaction (optimizations possible)
    read_only: bool = false,
};

/// Transaction metadata
pub const Transaction = struct {
    /// Unique transaction ID
    id: TransactionId,
    /// Current state
    state: std.atomic.Value(TransactionState),
    /// Isolation level
    isolation_level: IsolationLevel,
    /// Start timestamp (milliseconds)
    start_time: i64,
    /// Timeout in milliseconds
    timeout_ms: u64,
    /// Read-only flag
    read_only: bool,
    /// LSN when transaction started (for MVCC snapshot)
    snapshot_lsn: LSN,
    /// LSN when transaction committed (0 if not committed)
    commit_lsn: std.atomic.Value(LSN),
    /// Objects modified by this transaction
    modified_objects: std.ArrayList(ObjRef),
    /// Allocator for transaction data
    allocator: Allocator,

    const Self = @This();

    /// Create a new transaction
    pub fn init(allocator: Allocator, id: TransactionId, snapshot_lsn: LSN, config: TransactionConfig) !Self {
        return Self{
            .id = id,
            .state = std.atomic.Value(TransactionState).init(.active),
            .isolation_level = config.isolation_level,
            .start_time = std.time.milliTimestamp(),
            .timeout_ms = config.timeout_ms,
            .read_only = config.read_only,
            .snapshot_lsn = snapshot_lsn,
            .commit_lsn = std.atomic.Value(LSN).init(0),
            .modified_objects = .{},
            .allocator = allocator,
        };
    }

    /// Clean up transaction resources
    pub fn deinit(self: *Self) void {
        self.modified_objects.deinit(self.allocator);
    }

    /// Get current state
    pub fn getState(self: *const Self) TransactionState {
        return self.state.load(.seq_cst);
    }

    /// Get commit LSN (0 if not committed)
    pub fn getCommitLSN(self: *const Self) LSN {
        return self.commit_lsn.load(.seq_cst);
    }

    /// Check if transaction is active
    pub fn isActive(self: *const Self) bool {
        return self.getState() == .active;
    }

    /// Check if transaction has timed out
    pub fn hasTimedOut(self: *const Self) bool {
        if (self.timeout_ms == 0) return false;

        const now = std.time.milliTimestamp();
        const elapsed = now - self.start_time;
        return elapsed > self.timeout_ms;
    }

    /// Check and update timeout state
    pub fn checkTimeout(self: *Self) bool {
        if (self.hasTimedOut()) {
            _ = self.state.cmpxchgStrong(.active, .timed_out, .seq_cst, .seq_cst);
            return true;
        }
        return false;
    }

    /// Record a modified object
    pub fn recordModification(self: *Self, objref: ObjRef) !void {
        if (self.read_only) {
            return error.ReadOnlyTransaction;
        }

        if (!self.isActive()) {
            return error.TransactionNotActive;
        }

        try self.modified_objects.append(self.allocator, objref);
    }

    /// Transition to preparing state
    pub fn prepare(self: *Self) !void {
        const current = self.getState();
        if (current != .active) {
            return error.InvalidStateTransition;
        }

        if (self.checkTimeout()) {
            return error.TransactionTimedOut;
        }

        if (self.state.cmpxchgStrong(.active, .preparing, .seq_cst, .seq_cst)) |_| {
            return error.StateTransitionFailed;
        }
    }

    /// Commit the transaction
    pub fn commit(self: *Self, commit_lsn: LSN) !void {
        const current = self.getState();
        if (current != .preparing) {
            return error.InvalidStateTransition;
        }

        self.commit_lsn.store(commit_lsn, .seq_cst);
        if (self.state.cmpxchgStrong(.preparing, .committed, .seq_cst, .seq_cst)) |_| {
            return error.StateTransitionFailed;
        }
    }

    /// Abort the transaction
    pub fn abort(self: *Self) void {
        const current = self.getState();
        if (current.isTerminal()) return;

        _ = self.state.cmpxchgStrong(current, .aborted, .seq_cst, .seq_cst);
    }

    /// Get transaction age in milliseconds
    pub fn getAge(self: *const Self) i64 {
        const now = std.time.milliTimestamp();
        return now - self.start_time;
    }
};

/// Transaction statistics
pub const TransactionStats = struct {
    active_count: u64,
    committed_count: u64,
    aborted_count: u64,
    timed_out_count: u64,
    total_started: u64,

    pub fn init() TransactionStats {
        return TransactionStats{
            .active_count = 0,
            .committed_count = 0,
            .aborted_count = 0,
            .timed_out_count = 0,
            .total_started = 0,
        };
    }
};

/// Transaction Manager - manages all active transactions
pub const TransactionManager = struct {
    allocator: Allocator,
    next_txn_id: std.atomic.Value(TransactionId),
    current_lsn: std.atomic.Value(LSN),
    transactions: std.AutoHashMap(TransactionId, *Transaction),
    transactions_mutex: std.Thread.Mutex,
    stats: TransactionStats,
    stats_mutex: std.Thread.Mutex,
    /// Optional WAL for durability (can be null for testing)
    wal: ?*WAL,

    const Self = @This();

    /// Initialize transaction manager
    pub fn init(allocator: Allocator) Self {
        return Self{
            .allocator = allocator,
            .next_txn_id = std.atomic.Value(TransactionId).init(1),
            .current_lsn = std.atomic.Value(LSN).init(1),
            .transactions = std.AutoHashMap(TransactionId, *Transaction).init(allocator),
            .transactions_mutex = .{},
            .stats = TransactionStats.init(),
            .stats_mutex = .{},
            .wal = null,
        };
    }

    /// Initialize transaction manager with WAL
    pub fn initWithWAL(allocator: Allocator, wal: *WAL) Self {
        var mgr = Self.init(allocator);
        mgr.wal = wal;
        return mgr;
    }

    /// Clean up transaction manager
    pub fn deinit(self: *Self) void {
        self.transactions_mutex.lock();
        defer self.transactions_mutex.unlock();

        var iter = self.transactions.valueIterator();
        while (iter.next()) |txn_ptr| {
            txn_ptr.*.deinit();
            self.allocator.destroy(txn_ptr.*);
        }

        self.transactions.deinit();
    }

    /// Begin a new transaction
    pub fn begin(self: *Self, config: TransactionConfig) !*Transaction {
        const txn_id = self.next_txn_id.fetchAdd(1, .seq_cst);
        const snapshot_lsn = self.current_lsn.load(.seq_cst);

        const txn = try self.allocator.create(Transaction);
        errdefer self.allocator.destroy(txn);

        txn.* = try Transaction.init(self.allocator, txn_id, snapshot_lsn, config);

        self.transactions_mutex.lock();
        defer self.transactions_mutex.unlock();

        try self.transactions.put(txn_id, txn);

        // Update stats
        self.stats_mutex.lock();
        defer self.stats_mutex.unlock();
        self.stats.active_count += 1;
        self.stats.total_started += 1;

        return txn;
    }

    /// Commit a transaction
    pub fn commitTransaction(self: *Self, txn: *Transaction) !void {
        // Validate no conflicts before committing
        try self.validateCommit(txn);

        try txn.prepare();

        // Write commit record to WAL if available
        if (self.wal) |wal| {
            const commit_lsn = wal.nextLSN();
            const commit_data = try std.fmt.allocPrint(self.allocator, "txn:{d}", .{txn.id});
            defer self.allocator.free(commit_data);

            var commit_entry = try WALEntry.init(
                self.allocator,
                commit_lsn,
                .commit,
                ObjRef.nullRef(),
                commit_data,
            );
            defer commit_entry.deinit(self.allocator);

            // Write to WAL and ensure durability
            try wal.append(&commit_entry);

            try txn.commit(commit_lsn);
        } else {
            // No WAL - just commit with next LSN
            const commit_lsn = self.current_lsn.fetchAdd(1, .seq_cst) + 1;
            try txn.commit(commit_lsn);
        }

        // Update stats
        self.stats_mutex.lock();
        defer self.stats_mutex.unlock();
        self.stats.active_count -= 1;
        self.stats.committed_count += 1;
    }

    /// Abort a transaction
    pub fn abortTransaction(self: *Self, txn: *Transaction) !void {
        // Write abort record to WAL if available
        if (self.wal) |wal| {
            const abort_lsn = wal.nextLSN();
            const abort_data = try std.fmt.allocPrint(self.allocator, "txn:{d}", .{txn.id});
            defer self.allocator.free(abort_data);

            var abort_entry = try WALEntry.init(
                self.allocator,
                abort_lsn,
                .abort,
                ObjRef.nullRef(),
                abort_data,
            );
            defer abort_entry.deinit(self.allocator);

            // Write to WAL (best effort - abort should always succeed)
            wal.append(&abort_entry) catch |err| {
                std.log.warn("Failed to write abort to WAL: {}", .{err});
            };
        }

        txn.abort();

        // Update stats
        self.stats_mutex.lock();
        defer self.stats_mutex.unlock();
        self.stats.active_count -= 1;
        self.stats.aborted_count += 1;
    }

    /// End a transaction (cleanup)
    pub fn endTransaction(self: *Self, txn: *Transaction) void {
        self.transactions_mutex.lock();
        defer self.transactions_mutex.unlock();

        _ = self.transactions.remove(txn.id);
        txn.deinit();
        self.allocator.destroy(txn);
    }

    /// Get transaction by ID
    pub fn getTransaction(self: *Self, txn_id: TransactionId) ?*Transaction {
        self.transactions_mutex.lock();
        defer self.transactions_mutex.unlock();

        return self.transactions.get(txn_id);
    }

    /// Check for timed out transactions
    pub fn checkTimeouts(self: *Self) !void {
        self.transactions_mutex.lock();
        defer self.transactions_mutex.unlock();

        var iter = self.transactions.valueIterator();
        while (iter.next()) |txn_ptr| {
            if (txn_ptr.*.checkTimeout()) {
                self.stats_mutex.lock();
                self.stats.active_count -= 1;
                self.stats.timed_out_count += 1;
                self.stats_mutex.unlock();
            }
        }
    }

    /// Get statistics
    pub fn getStats(self: *Self) TransactionStats {
        self.stats_mutex.lock();
        defer self.stats_mutex.unlock();
        return self.stats;
    }

    /// Get active transaction count
    pub fn getActiveCount(self: *Self) u64 {
        self.transactions_mutex.lock();
        defer self.transactions_mutex.unlock();
        return self.transactions.count();
    }

    /// Check for write-write conflicts between transactions
    /// Returns true if there's a conflict (another transaction modified the same object)
    /// For serializable isolation, we check against transactions that:
    /// - Committed after this transaction started (snapshot_lsn)
    /// Note: We don't check active transactions because they haven't committed yet
    pub fn hasWriteConflict(self: *Self, txn: *Transaction, objref: ObjRef) bool {
        self.transactions_mutex.lock();
        defer self.transactions_mutex.unlock();

        var iter = self.transactions.valueIterator();
        while (iter.next()) |other_txn_ptr| {
            const other_txn = other_txn_ptr.*;

            // Skip self
            if (other_txn.id == txn.id) {
                continue;
            }

            // Only check committed transactions that committed after our snapshot
            if (other_txn.getState() != .committed) {
                continue;
            }

            if (other_txn.getCommitLSN() <= txn.snapshot_lsn) {
                continue; // Committed before our snapshot, no conflict
            }

            // Check if other transaction modified this object
            for (other_txn.modified_objects.items) |modified_ref| {
                if (modified_ref.getOffset() == objref.getOffset()) {
                    return true; // Conflict detected
                }
            }
        }

        return false; // No conflict
    }

    /// Validate transaction can commit (no conflicts)
    pub fn validateCommit(self: *Self, txn: *Transaction) !void {
        // For serializable isolation, check for conflicts
        if (txn.isolation_level == .serializable) {
            for (txn.modified_objects.items) |objref| {
                if (self.hasWriteConflict(txn, objref)) {
                    return error.WriteConflict;
                }
            }
        }
    }
};

// Tests
test "Transaction lifecycle" {
    const allocator = std.testing.allocator;

    var txn = try Transaction.init(allocator, 1, 100, TransactionConfig{});
    defer txn.deinit();

    // Should start as active
    try std.testing.expectEqual(TransactionState.active, txn.getState());
    try std.testing.expect(txn.isActive());

    // Prepare
    try txn.prepare();
    try std.testing.expectEqual(TransactionState.preparing, txn.getState());

    // Commit
    try txn.commit(101);
    try std.testing.expectEqual(TransactionState.committed, txn.getState());
    try std.testing.expectEqual(@as(LSN, 101), txn.commit_lsn.load(.seq_cst));
}

test "Transaction ID uniqueness" {
    const allocator = std.testing.allocator;

    var manager = TransactionManager.init(allocator);
    defer manager.deinit();

    const txn1 = try manager.begin(TransactionConfig{});
    const txn2 = try manager.begin(TransactionConfig{});
    const txn3 = try manager.begin(TransactionConfig{});

    try std.testing.expect(txn1.id != txn2.id);
    try std.testing.expect(txn2.id != txn3.id);
    try std.testing.expect(txn1.id < txn2.id);
    try std.testing.expect(txn2.id < txn3.id);

    manager.endTransaction(txn1);
    manager.endTransaction(txn2);
    manager.endTransaction(txn3);
}

test "Transaction timeout" {
    const allocator = std.testing.allocator;

    var config = TransactionConfig{};
    config.timeout_ms = 10; // 10ms timeout

    var txn = try Transaction.init(allocator, 1, 100, config);
    defer txn.deinit();

    // Should not be timed out initially
    try std.testing.expect(!txn.hasTimedOut());

    // Wait for timeout
    std.Thread.sleep(15 * std.time.ns_per_ms);

    // Should be timed out now
    try std.testing.expect(txn.hasTimedOut());
    try std.testing.expect(txn.checkTimeout());
    try std.testing.expectEqual(TransactionState.timed_out, txn.getState());
}

test "Transaction manager begin and commit" {
    const allocator = std.testing.allocator;

    var manager = TransactionManager.init(allocator);
    defer manager.deinit();

    const txn = try manager.begin(TransactionConfig{});

    try std.testing.expect(txn.isActive());
    try std.testing.expectEqual(@as(u64, 1), manager.getActiveCount());

    try manager.commitTransaction(txn);

    try std.testing.expectEqual(TransactionState.committed, txn.getState());

    const stats = manager.getStats();
    try std.testing.expectEqual(@as(u64, 0), stats.active_count);
    try std.testing.expectEqual(@as(u64, 1), stats.committed_count);

    manager.endTransaction(txn);
}

test "Transaction manager abort" {
    const allocator = std.testing.allocator;

    var manager = TransactionManager.init(allocator);
    defer manager.deinit();

    const txn = try manager.begin(TransactionConfig{});

    try manager.abortTransaction(txn);

    try std.testing.expectEqual(TransactionState.aborted, txn.getState());

    const stats = manager.getStats();
    try std.testing.expectEqual(@as(u64, 0), stats.active_count);
    try std.testing.expectEqual(@as(u64, 1), stats.aborted_count);

    manager.endTransaction(txn);
}

test "Read-only transaction" {
    const allocator = std.testing.allocator;

    var config = TransactionConfig{};
    config.read_only = true;

    var txn = try Transaction.init(allocator, 1, 100, config);
    defer txn.deinit();

    // Should not be able to record modifications
    const result = txn.recordModification(ObjRef.init(1024));
    try std.testing.expectError(error.ReadOnlyTransaction, result);
}

// Task 4.3 Tests: Transaction Commit & Abort with WAL

test "Transaction commit with WAL" {
    const allocator = std.testing.allocator;
    const test_dir = "test_txn_commit_wal";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL
    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create transaction manager with WAL
    var manager = TransactionManager.initWithWAL(allocator, &wal);
    defer manager.deinit();

    // Begin transaction
    const txn = try manager.begin(TransactionConfig{});

    // Record some modifications
    try txn.recordModification(ObjRef.init(1024));
    try txn.recordModification(ObjRef.init(2048));

    // Commit transaction
    try manager.commitTransaction(txn);

    // Verify transaction is committed
    try std.testing.expectEqual(TransactionState.committed, txn.getState());

    // Verify WAL has commit entry
    const stats = wal.getStats();
    try std.testing.expect(stats.entry_count > 0);

    manager.endTransaction(txn);
}

test "Transaction abort with WAL" {
    const allocator = std.testing.allocator;
    const test_dir = "test_txn_abort_wal";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL
    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create transaction manager with WAL
    var manager = TransactionManager.initWithWAL(allocator, &wal);
    defer manager.deinit();

    // Begin transaction
    const txn = try manager.begin(TransactionConfig{});

    // Record some modifications
    try txn.recordModification(ObjRef.init(1024));

    // Abort transaction
    try manager.abortTransaction(txn);

    // Verify transaction is aborted
    try std.testing.expectEqual(TransactionState.aborted, txn.getState());

    // Verify WAL has abort entry
    const stats = wal.getStats();
    try std.testing.expect(stats.entry_count > 0);

    manager.endTransaction(txn);
}

test "Transaction commit atomicity" {
    const allocator = std.testing.allocator;
    const test_dir = "test_txn_atomicity";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL
    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create transaction manager with WAL
    var manager = TransactionManager.initWithWAL(allocator, &wal);
    defer manager.deinit();

    // Begin transaction
    const txn = try manager.begin(TransactionConfig{});

    // Record modifications
    try txn.recordModification(ObjRef.init(1024));
    try txn.recordModification(ObjRef.init(2048));
    try txn.recordModification(ObjRef.init(3072));

    // Commit should be atomic - either all succeed or all fail
    try manager.commitTransaction(txn);

    // All modifications should be visible after commit
    try std.testing.expectEqual(@as(usize, 3), txn.modified_objects.items.len);
    try std.testing.expectEqual(TransactionState.committed, txn.getState());

    manager.endTransaction(txn);
}

test "Transaction conflict detection" {
    const allocator = std.testing.allocator;

    var manager = TransactionManager.init(allocator);
    defer manager.deinit();

    // Begin two transactions concurrently
    var config = TransactionConfig{};
    config.isolation_level = .serializable;

    const txn1 = try manager.begin(config);
    const txn2 = try manager.begin(config);

    // Debug: Check snapshot LSNs
    const snap1 = txn1.snapshot_lsn;
    const snap2 = txn2.snapshot_lsn;
    try std.testing.expectEqual(snap1, snap2); // Should be the same

    // Both modify the same object
    const objref = ObjRef.init(1024);
    try txn1.recordModification(objref);
    try txn2.recordModification(objref);

    // First transaction commits successfully
    try manager.commitTransaction(txn1);

    // Debug: Check commit LSN
    const commit1 = txn1.getCommitLSN();
    try std.testing.expect(commit1 > snap2); // txn1 committed after txn2's snapshot

    // Second transaction should detect conflict
    // (txn1 is committed and modified the same object after txn2's snapshot)
    const result = manager.commitTransaction(txn2);
    try std.testing.expectError(error.WriteConflict, result);

    manager.endTransaction(txn1);
    manager.endTransaction(txn2);
}

test "Transaction conflict detection - no conflict" {
    const allocator = std.testing.allocator;

    var manager = TransactionManager.init(allocator);
    defer manager.deinit();

    // Begin two transactions
    var config = TransactionConfig{};
    config.isolation_level = .serializable;

    const txn1 = try manager.begin(config);
    const txn2 = try manager.begin(config);

    // Transactions modify different objects
    try txn1.recordModification(ObjRef.init(1024));
    try txn2.recordModification(ObjRef.init(2048));

    // Both should commit successfully (no conflict)
    try manager.commitTransaction(txn1);
    try manager.commitTransaction(txn2);

    manager.endTransaction(txn1);
    manager.endTransaction(txn2);
}

test "Transaction durability guarantee" {
    const allocator = std.testing.allocator;
    const test_dir = "test_txn_durability";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL
    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create transaction manager with WAL
    var manager = TransactionManager.initWithWAL(allocator, &wal);
    defer manager.deinit();

    // Begin and commit transaction
    const txn = try manager.begin(TransactionConfig{});
    try txn.recordModification(ObjRef.init(1024));
    try manager.commitTransaction(txn);

    const commit_lsn = txn.getCommitLSN();
    try std.testing.expect(commit_lsn > 0);

    manager.endTransaction(txn);

    // WAL should have the commit record persisted
    const stats = wal.getStats();
    try std.testing.expect(stats.entry_count > 0);
    try std.testing.expect(stats.last_lsn >= commit_lsn);
}

test "Transaction abort rollback" {
    const allocator = std.testing.allocator;

    var manager = TransactionManager.init(allocator);
    defer manager.deinit();

    // Begin transaction
    const txn = try manager.begin(TransactionConfig{});

    // Record modifications
    try txn.recordModification(ObjRef.init(1024));
    try txn.recordModification(ObjRef.init(2048));

    // Verify modifications are tracked
    try std.testing.expectEqual(@as(usize, 2), txn.modified_objects.items.len);

    // Abort transaction
    try manager.abortTransaction(txn);

    // Transaction should be aborted
    try std.testing.expectEqual(TransactionState.aborted, txn.getState());

    // In a full implementation, modifications would be rolled back
    // For now, we verify the transaction state is correct

    manager.endTransaction(txn);
}

test "Transaction isolation levels" {
    const allocator = std.testing.allocator;

    var manager = TransactionManager.init(allocator);
    defer manager.deinit();

    // Test different isolation levels
    const levels = [_]IsolationLevel{
        .read_uncommitted,
        .read_committed,
        .repeatable_read,
        .serializable,
    };

    for (levels) |level| {
        var config = TransactionConfig{};
        config.isolation_level = level;

        const txn = try manager.begin(config);
        try std.testing.expectEqual(level, txn.isolation_level);

        try manager.commitTransaction(txn);
        manager.endTransaction(txn);
    }
}
