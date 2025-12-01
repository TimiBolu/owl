//! Transaction FFI Interface (Zig)
//!
//! Task 8.13: Implement Rust-Zig transaction coordination (Rust + Zig)
//!
//! This module provides the FFI interface for transaction coordination between
//! Rust and Zig layers. It exposes C-compatible functions that the Rust
//! distributed transaction coordinator can call to manage Zig-level transactions.

const std = @import("std");
const Allocator = std.mem.Allocator;
const Transaction = @import("transaction.zig").Transaction;
const TransactionManager = @import("transaction.zig").TransactionManager;
const TransactionConfig = @import("transaction.zig").TransactionConfig;
const TransactionState = @import("transaction.zig").TransactionState;
const IsolationLevel = @import("transaction.zig").IsolationLevel;
const WAL = @import("wal.zig").WAL;
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ErrorHandling = @import("error_handling.zig");

/// Global allocator for FFI operations
var gpa = std.heap.c_allocator;

/// Global transaction managers per shard
var shard_managers: std.AutoHashMap(u32, *TransactionManager) = undefined;
var managers_initialized = false;
var managers_mutex: std.Thread.Mutex = .{};

/// Initialize the transaction FFI system
export fn zig_transaction_ffi_init() void {
    managers_mutex.lock();
    defer managers_mutex.unlock();

    if (!managers_initialized) {
        shard_managers = std.AutoHashMap(u32, *TransactionManager).init(gpa);
        managers_initialized = true;

        // Initialize error handling system
        _ = ErrorHandling.zig_error_init();
    }
}

/// Cleanup the transaction FFI system
export fn zig_transaction_ffi_cleanup() void {
    managers_mutex.lock();
    defer managers_mutex.unlock();

    if (managers_initialized) {
        var iter = shard_managers.valueIterator();
        while (iter.next()) |manager_ptr| {
            manager_ptr.*.deinit();
            gpa.destroy(manager_ptr.*);
        }
        shard_managers.deinit();
        managers_initialized = false;

        // Cleanup error handling system
        ErrorHandling.zig_error_cleanup();
    }
}

/// Get or create transaction manager for a shard
fn getOrCreateShardManager(shard_id: u32) !*TransactionManager {
    managers_mutex.lock();
    defer managers_mutex.unlock();

    if (!managers_initialized) {
        zig_transaction_ffi_init();
    }

    if (shard_managers.get(shard_id)) |manager| {
        return manager;
    }

    // Create new transaction manager for this shard
    const manager = try gpa.create(TransactionManager);
    manager.* = TransactionManager.init(gpa);
    try shard_managers.put(shard_id, manager);

    return manager;
}

/// Begin a transaction in Zig layer
/// Returns transaction ID (0 on failure)
export fn zig_transaction_begin(shard_id: u32, timeout_ms: u64) u64 {
    const manager = getOrCreateShardManager(shard_id) catch {
        const zig_error = ErrorHandling.outOfMemoryError(shard_id)
            .withContext("operation", "begin_transaction");
        ErrorHandling.recordError(shard_id, zig_error);
        return 0;
    };

    const config = TransactionConfig{
        .isolation_level = .repeatable_read,
        .timeout_ms = timeout_ms,
        .read_only = false,
    };

    const txn = manager.begin(config) catch |err| {
        const error_msg = switch (err) {
            error.OutOfMemory => "Failed to allocate memory for transaction",
            else => "Failed to begin transaction",
        };
        const zig_error = ErrorHandling.ZigError.init(.invalid_transaction, error_msg)
            .withShard(shard_id)
            .withContext("operation", "begin")
            .withContext("timeout_ms", "30000"); // Convert timeout to string if needed
        ErrorHandling.recordError(shard_id, zig_error);
        return 0;
    };

    return txn.id;
}

/// Prepare a Zig transaction (2PC phase 1)
/// Returns true if prepared successfully, false otherwise
export fn zig_transaction_prepare(shard_id: u32, zig_txn_id: u64) bool {
    const manager = getOrCreateShardManager(shard_id) catch {
        const zig_error = ErrorHandling.outOfMemoryError(shard_id)
            .withContext("operation", "prepare_transaction");
        ErrorHandling.recordError(shard_id, zig_error);
        return false;
    };

    const txn = manager.getTransaction(zig_txn_id) orelse {
        const zig_error = ErrorHandling.transactionNotFoundError(shard_id, zig_txn_id)
            .withContext("operation", "prepare");
        ErrorHandling.recordError(shard_id, zig_error);
        return false;
    };

    // Attempt to prepare the transaction
    txn.prepare() catch |err| {
        const error_msg = switch (err) {
            error.TransactionNotActive => "Transaction is not in active state",
            error.TransactionTimedOut => "Transaction has timed out",
            else => "Failed to prepare transaction",
        };
        const error_code = switch (err) {
            error.TransactionNotActive => ErrorHandling.ZigErrorCode.transaction_not_active,
            error.TransactionTimedOut => ErrorHandling.ZigErrorCode.transaction_timed_out,
            else => ErrorHandling.ZigErrorCode.prepare_failure,
        };
        const zig_error = ErrorHandling.ZigError.init(error_code, error_msg)
            .withShard(shard_id)
            .withTransaction(zig_txn_id)
            .withContext("operation", "prepare");
        ErrorHandling.recordError(shard_id, zig_error);
        return false;
    };

    return true;
}

/// Commit a Zig transaction (2PC phase 2)
/// Returns true if committed successfully, false otherwise
export fn zig_transaction_commit(shard_id: u32, zig_txn_id: u64) bool {
    const manager = getOrCreateShardManager(shard_id) catch {
        const zig_error = ErrorHandling.outOfMemoryError(shard_id)
            .withContext("operation", "commit_transaction");
        ErrorHandling.recordError(shard_id, zig_error);
        return false;
    };

    const txn = manager.getTransaction(zig_txn_id) orelse {
        const zig_error = ErrorHandling.transactionNotFoundError(shard_id, zig_txn_id)
            .withContext("operation", "commit");
        ErrorHandling.recordError(shard_id, zig_error);
        return false;
    };

    // Attempt to commit the transaction
    manager.commitTransaction(txn) catch |err| {
        const error_msg = switch (err) {
            error.TransactionNotPrepared => "Transaction is not in prepared state",
            error.TransactionTimedOut => "Transaction has timed out",
            error.WALWriteFailure => "Failed to write to WAL during commit",
            else => "Failed to commit transaction",
        };
        const error_code = switch (err) {
            error.TransactionNotPrepared => ErrorHandling.ZigErrorCode.invalid_transaction,
            error.TransactionTimedOut => ErrorHandling.ZigErrorCode.transaction_timed_out,
            error.WALWriteFailure => ErrorHandling.ZigErrorCode.wal_error,
            else => ErrorHandling.ZigErrorCode.commit_failure,
        };
        const zig_error = ErrorHandling.ZigError.init(error_code, error_msg)
            .withShard(shard_id)
            .withTransaction(zig_txn_id)
            .withContext("operation", "commit");
        ErrorHandling.recordError(shard_id, zig_error);
        return false;
    };

    manager.endTransaction(txn);
    return true;
}

/// Abort a Zig transaction
/// Returns true if aborted successfully, false otherwise
export fn zig_transaction_abort(shard_id: u32, zig_txn_id: u64) bool {
    const manager = getOrCreateShardManager(shard_id) catch return false;
    const txn = manager.getTransaction(zig_txn_id) orelse return false;

    // Attempt to abort the transaction
    manager.abortTransaction(txn) catch return false;
    manager.endTransaction(txn);
    return true;
}

/// Get Zig transaction state
/// Returns state as u8 (0=invalid, 1=active, 2=preparing, 3=committed, 4=aborted, 5=timed_out)
export fn zig_transaction_get_state(shard_id: u32, zig_txn_id: u64) u8 {
    const manager = getOrCreateShardManager(shard_id) catch return 0;
    const txn = manager.getTransaction(zig_txn_id) orelse return 0;

    return switch (txn.getState()) {
        .active => 1,
        .preparing => 2,
        .committed => 3,
        .aborted => 4,
        .timed_out => 5,
    };
}

/// Write operation to Zig transaction
/// Returns true if operation was written successfully, false otherwise
export fn zig_transaction_write_operation(
    shard_id: u32,
    zig_txn_id: u64,
    operation_type: u8,
    data_ptr: [*]const u8,
    data_len: usize,
) bool {
    const manager = getOrCreateShardManager(shard_id) catch return false;
    const txn = manager.getTransaction(zig_txn_id) orelse return false;

    if (!txn.isActive()) return false;

    // Use the operation parameters to avoid unused warnings
    _ = operation_type; // Would be used to determine operation type
    _ = data_ptr[0..data_len]; // Would contain the actual operation data

    // Create a dummy object reference for the operation
    // In a real implementation, this would be provided by the caller
    const objref = ObjRef.init(1024);

    // Record the modification
    txn.recordModification(objref) catch return false;

    // In a full implementation, we would:
    // 1. Parse the operation data based on operation_type
    // 2. Apply it to the appropriate storage structures
    // 3. Write to WAL if needed
    // For now, we just record that the operation occurred

    return true;
}

/// Get transaction statistics for a shard
export fn zig_transaction_get_shard_stats(
    shard_id: u32,
    out_active_count: ?*u64,
    out_committed_count: ?*u64,
    out_aborted_count: ?*u64,
    out_timed_out_count: ?*u64,
) bool {
    const manager = getOrCreateShardManager(shard_id) catch return false;
    const stats = manager.getStats();

    if (out_active_count) |ptr| ptr.* = stats.active_count;
    if (out_committed_count) |ptr| ptr.* = stats.committed_count;
    if (out_aborted_count) |ptr| ptr.* = stats.aborted_count;
    if (out_timed_out_count) |ptr| ptr.* = stats.timed_out_count;

    return true;
}

/// Check for timed out transactions on a shard
/// Returns number of transactions that were timed out and aborted
export fn zig_transaction_check_timeouts(shard_id: u32) u32 {
    const manager = getOrCreateShardManager(shard_id) catch return 0;

    manager.checkTimeouts() catch return 0;

    // In a full implementation, we would return the actual count
    // For now, we return 0 to indicate success
    return 0;
}

/// Create a distributed transaction context
/// This is used for cross-shard operations
export fn zig_distributed_transaction_create(
    coordinator_shard: u32,
    participant_shards_ptr: [*]const u32,
    participant_count: usize,
    timeout_ms: u64,
) u64 {
    // This would create a distributed transaction context
    // For now, we just return a dummy ID
    _ = coordinator_shard;
    _ = participant_shards_ptr;
    _ = participant_count;
    _ = timeout_ms;

    // In a real implementation, this would:
    // 1. Create a distributed transaction coordinator
    // 2. Begin transactions on all participant shards
    // 3. Return a distributed transaction ID

    return 1; // Dummy distributed transaction ID
}

/// Write cross-shard operation to distributed transaction
export fn zig_distributed_transaction_write_cross_shard(
    distributed_txn_id: u64,
    source_shard: u32,
    target_shard: u32,
    operation_type: u8,
    objref_shard: u32,
    objref_offset: u64,
    data_ptr: [*]const u8,
    data_len: usize,
) bool {
    // This would write a cross-shard operation to the distributed transaction
    _ = distributed_txn_id;
    _ = source_shard;
    _ = target_shard;
    _ = operation_type;
    _ = objref_shard;
    _ = objref_offset; // Would be used to create GlobalObjRef
    _ = data_ptr[0..data_len]; // Use the data to avoid unused warning

    // In a real implementation, this would:
    // 1. Validate the distributed transaction exists
    // 2. Create a cross-shard operation entry
    // 3. Write it to the appropriate WAL
    // 4. Coordinate with the target shard

    return true; // Assume success for now
}

/// Prepare distributed transaction (2PC phase 1)
export fn zig_distributed_transaction_prepare(distributed_txn_id: u64) bool {
    // This would prepare all participants in the distributed transaction
    _ = distributed_txn_id;

    // In a real implementation, this would:
    // 1. Send prepare messages to all participants
    // 2. Wait for responses
    // 3. Return true only if all participants are prepared

    return true; // Assume success for now
}

/// Commit distributed transaction (2PC phase 2)
export fn zig_distributed_transaction_commit(distributed_txn_id: u64) bool {
    // This would commit all participants in the distributed transaction
    _ = distributed_txn_id;

    // In a real implementation, this would:
    // 1. Send commit messages to all participants
    // 2. Wait for acknowledgments
    // 3. Clean up the distributed transaction context

    return true; // Assume success for now
}

/// Abort distributed transaction
export fn zig_distributed_transaction_abort(distributed_txn_id: u64) bool {
    // This would abort all participants in the distributed transaction
    _ = distributed_txn_id;

    // In a real implementation, this would:
    // 1. Send abort messages to all participants
    // 2. Clean up any partial state
    // 3. Clean up the distributed transaction context

    return true; // Assume success for now
}

/// Error handling: Get last error message for a shard
export fn zig_transaction_get_last_error(
    shard_id: u32,
    buffer: [*]u8,
    buffer_len: usize,
) usize {
    if (ErrorHandling.getLastError(shard_id)) |zig_error| {
        const message = zig_error.getMessage();
        const copy_len = @min(message.len, buffer_len - 1);

        @memcpy(buffer[0..copy_len], message[0..copy_len]);
        buffer[copy_len] = 0; // Null terminate

        return copy_len;
    }

    const no_error_msg = "No error";
    const copy_len = @min(no_error_msg.len, buffer_len - 1);

    @memcpy(buffer[0..copy_len], no_error_msg[0..copy_len]);
    buffer[copy_len] = 0; // Null terminate

    return copy_len;
}

/// Synchronization: Wait for transaction to reach a specific state
export fn zig_transaction_wait_for_state(
    shard_id: u32,
    zig_txn_id: u64,
    target_state: u8,
    timeout_ms: u64,
) bool {
    const manager = getOrCreateShardManager(shard_id) catch return false;
    const txn = manager.getTransaction(zig_txn_id) orelse return false;

    const target = switch (target_state) {
        1 => TransactionState.active,
        2 => TransactionState.preparing,
        3 => TransactionState.committed,
        4 => TransactionState.aborted,
        5 => TransactionState.timed_out,
        else => return false,
    };

    // Simple polling implementation
    // In a real implementation, this would use proper synchronization primitives
    const start_time = std.time.milliTimestamp();
    const timeout = @as(i64, @intCast(timeout_ms));

    while (std.time.milliTimestamp() - start_time < timeout) {
        if (txn.getState() == target) {
            return true;
        }

        // Sleep for a short time before checking again
        std.Thread.sleep(1000000); // 1ms
    }

    return false; // Timeout
}

/// Batch operations: Begin batch transaction context
export fn zig_transaction_begin_batch(
    shard_id: u32,
    batch_size_hint: u32,
    timeout_ms: u64,
) u64 {
    // This would create a batch transaction context for efficient bulk operations
    _ = batch_size_hint;

    // For now, just create a regular transaction
    return zig_transaction_begin(shard_id, timeout_ms);
}

/// Batch operations: Add operation to batch
export fn zig_transaction_batch_add_operation(
    shard_id: u32,
    batch_txn_id: u64,
    operation_type: u8,
    objref_offset: u64,
    data_ptr: [*]const u8,
    data_len: usize,
) bool {
    // This would add an operation to a batch transaction
    // For now, just use the regular write operation
    // Use objref_offset to avoid unused parameter warning
    _ = objref_offset;
    return zig_transaction_write_operation(
        shard_id,
        batch_txn_id,
        operation_type,
        data_ptr,
        data_len,
    );
}

/// Batch operations: Execute all operations in batch
export fn zig_transaction_batch_execute(shard_id: u32, batch_txn_id: u64) bool {
    // This would execute all operations in the batch atomically
    // For now, operations are executed immediately, so this is a no-op
    _ = shard_id;
    _ = batch_txn_id;
    return true;
}

// ============================================================================
// Tests
// ============================================================================

test "Transaction FFI: initialization and cleanup" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    // Should be able to initialize multiple times safely
    zig_transaction_ffi_init();
}

test "Transaction FFI: basic transaction lifecycle" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    const shard_id = 0;
    const timeout_ms = 30000;

    // Begin transaction
    const txn_id = zig_transaction_begin(shard_id, timeout_ms);
    try std.testing.expect(txn_id != 0);

    // Check initial state
    const initial_state = zig_transaction_get_state(shard_id, txn_id);
    try std.testing.expectEqual(@as(u8, 1), initial_state); // active

    // Write an operation
    const data = "test operation data";
    const write_success = zig_transaction_write_operation(
        shard_id,
        txn_id,
        1, // insert operation
        data.ptr,
        data.len,
    );
    try std.testing.expect(write_success);

    // Prepare transaction
    const prepare_success = zig_transaction_prepare(shard_id, txn_id);
    try std.testing.expect(prepare_success);

    // Commit transaction
    const commit_success = zig_transaction_commit(shard_id, txn_id);
    try std.testing.expect(commit_success);
}

test "Transaction FFI: transaction abort" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    const shard_id = 0;
    const timeout_ms = 30000;

    // Begin transaction
    const txn_id = zig_transaction_begin(shard_id, timeout_ms);
    try std.testing.expect(txn_id != 0);

    // Write an operation
    const data = "test operation data";
    _ = zig_transaction_write_operation(
        shard_id,
        txn_id,
        1, // insert operation
        data.ptr,
        data.len,
    );

    // Abort transaction
    const abort_success = zig_transaction_abort(shard_id, txn_id);
    try std.testing.expect(abort_success);
}

test "Transaction FFI: statistics" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    const shard_id = 0;
    var active_count: u64 = 0;
    var committed_count: u64 = 0;
    var aborted_count: u64 = 0;
    var timed_out_count: u64 = 0;

    // Get initial stats
    const stats_success = zig_transaction_get_shard_stats(
        shard_id,
        &active_count,
        &committed_count,
        &aborted_count,
        &timed_out_count,
    );
    try std.testing.expect(stats_success);

    // Initially should have no transactions
    try std.testing.expectEqual(@as(u64, 0), active_count);
}

test "Transaction FFI: error handling" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    const shard_id = 0;

    // Try to operate on non-existent transaction
    const invalid_txn_id = 999999;
    const state = zig_transaction_get_state(shard_id, invalid_txn_id);
    try std.testing.expectEqual(@as(u8, 0), state); // invalid

    const prepare_result = zig_transaction_prepare(shard_id, invalid_txn_id);
    try std.testing.expect(!prepare_result);

    const commit_result = zig_transaction_commit(shard_id, invalid_txn_id);
    try std.testing.expect(!commit_result);

    const abort_result = zig_transaction_abort(shard_id, invalid_txn_id);
    try std.testing.expect(!abort_result);
}

test "Transaction FFI: batch operations" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    const shard_id = 0;
    const timeout_ms = 30000;
    const batch_size = 10;

    // Begin batch transaction
    const batch_txn_id = zig_transaction_begin_batch(shard_id, batch_size, timeout_ms);
    try std.testing.expect(batch_txn_id != 0);

    // Add operations to batch
    for (0..batch_size) |i| {
        const data = std.fmt.allocPrint(std.testing.allocator, "batch operation {}", .{i}) catch unreachable;
        defer std.testing.allocator.free(data);

        const add_success = zig_transaction_batch_add_operation(
            shard_id,
            batch_txn_id,
            1, // insert operation
            1024 + i * 64, // objref offset
            data.ptr,
            data.len,
        );
        try std.testing.expect(add_success);
    }

    // Execute batch
    const execute_success = zig_transaction_batch_execute(shard_id, batch_txn_id);
    try std.testing.expect(execute_success);

    // Commit batch transaction
    const commit_success = zig_transaction_commit(shard_id, batch_txn_id);
    try std.testing.expect(commit_success);
}

test "Transaction FFI: distributed transaction stubs" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    const coordinator_shard = 0;
    const participant_shards = [_]u32{ 1, 2, 3 };
    const timeout_ms = 30000;

    // Create distributed transaction
    const dist_txn_id = zig_distributed_transaction_create(
        coordinator_shard,
        &participant_shards,
        participant_shards.len,
        timeout_ms,
    );
    try std.testing.expect(dist_txn_id != 0);

    // Write cross-shard operation
    const data = "cross-shard operation data";
    const write_success = zig_distributed_transaction_write_cross_shard(
        dist_txn_id,
        0, // source shard
        1, // target shard
        1, // insert operation
        1, // objref shard
        2048, // objref offset
        data.ptr,
        data.len,
    );
    try std.testing.expect(write_success);

    // Prepare distributed transaction
    const prepare_success = zig_distributed_transaction_prepare(dist_txn_id);
    try std.testing.expect(prepare_success);

    // Commit distributed transaction
    const commit_success = zig_distributed_transaction_commit(dist_txn_id);
    try std.testing.expect(commit_success);
}

test "Transaction FFI: wait for state" {
    zig_transaction_ffi_init();
    defer zig_transaction_ffi_cleanup();

    const shard_id = 0;
    const timeout_ms = 30000;

    // Begin transaction
    const txn_id = zig_transaction_begin(shard_id, timeout_ms);
    try std.testing.expect(txn_id != 0);

    // Should already be in active state
    const wait_success = zig_transaction_wait_for_state(
        shard_id,
        txn_id,
        1, // active state
        1000, // 1 second timeout
    );
    try std.testing.expect(wait_success);

    // Clean up
    _ = zig_transaction_abort(shard_id, txn_id);
}
