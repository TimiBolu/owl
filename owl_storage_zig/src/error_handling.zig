//! Cross-Layer Error Handling (Zig)
//!
//! Task 8.14: Add cross-layer error handling (Rust + Zig)
//!
//! This module provides structured error handling for the Zig storage layer,
//! with proper error propagation to the Rust layer through FFI.

const std = @import("std");
const Allocator = std.mem.Allocator;

/// Maximum error message length for FFI
pub const MAX_ERROR_MESSAGE_LEN = 1024;

/// Maximum error context entries
pub const MAX_ERROR_CONTEXT_ENTRIES = 16;

/// Error severity levels matching Rust ErrorSeverity
pub const ErrorSeverity = enum(u8) {
    warning = 0, // Non-fatal, operation can continue
    err = 1, // Fatal for current operation, but system can recover (renamed from 'error' to avoid keyword conflict)
    critical = 2, // System-level error requiring immediate attention
};

/// Error codes that can be returned to Rust layer
pub const ZigErrorCode = enum(c_int) {
    success = 0,
    out_of_memory = -1,
    invalid_transaction = -2,
    transaction_not_found = -3,
    transaction_not_active = -4,
    transaction_timed_out = -5,
    prepare_failure = -6,
    commit_failure = -7,
    abort_failure = -8,
    invalid_operation = -9,
    invalid_shard = -10,
    shard_not_found = -11,
    wal_error = -12,
    storage_error = -13,
    network_error = -14,
    serialization_error = -15,
    deserialization_error = -16,
    checksum_mismatch = -17,
    corrupted_data = -18,
    insufficient_space = -19,
    lock_conflict = -20,
    deadlock_detected = -21,
    unknown = -999,

    /// Convert to C integer
    pub fn toInt(self: ZigErrorCode) c_int {
        return @intFromEnum(self);
    }

    /// Create from C integer
    pub fn fromInt(code: c_int) ZigErrorCode {
        return switch (code) {
            0 => .success,
            -1 => .out_of_memory,
            -2 => .invalid_transaction,
            -3 => .transaction_not_found,
            -4 => .transaction_not_active,
            -5 => .transaction_timed_out,
            -6 => .prepare_failure,
            -7 => .commit_failure,
            -8 => .abort_failure,
            -9 => .invalid_operation,
            -10 => .invalid_shard,
            -11 => .shard_not_found,
            -12 => .wal_error,
            -13 => .storage_error,
            -14 => .network_error,
            -15 => .serialization_error,
            -16 => .deserialization_error,
            -17 => .checksum_mismatch,
            -18 => .corrupted_data,
            -19 => .insufficient_space,
            -20 => .lock_conflict,
            -21 => .deadlock_detected,
            else => .unknown,
        };
    }

    /// Get human-readable description
    pub fn description(self: ZigErrorCode) []const u8 {
        return switch (self) {
            .success => "Operation completed successfully",
            .out_of_memory => "Out of memory",
            .invalid_transaction => "Invalid transaction",
            .transaction_not_found => "Transaction not found",
            .transaction_not_active => "Transaction is not active",
            .transaction_timed_out => "Transaction timed out",
            .prepare_failure => "Transaction prepare failed",
            .commit_failure => "Transaction commit failed",
            .abort_failure => "Transaction abort failed",
            .invalid_operation => "Invalid operation",
            .invalid_shard => "Invalid shard",
            .shard_not_found => "Shard not found",
            .wal_error => "Write-ahead log error",
            .storage_error => "Storage layer error",
            .network_error => "Network communication error",
            .serialization_error => "Data serialization error",
            .deserialization_error => "Data deserialization error",
            .checksum_mismatch => "Data checksum mismatch",
            .corrupted_data => "Corrupted data detected",
            .insufficient_space => "Insufficient storage space",
            .lock_conflict => "Lock conflict detected",
            .deadlock_detected => "Deadlock detected",
            .unknown => "Unknown error",
        };
    }

    /// Check if this represents a success
    pub fn isSuccess(self: ZigErrorCode) bool {
        return self == .success;
    }

    /// Check if this represents a transient error that might be retryable
    pub fn isTransient(self: ZigErrorCode) bool {
        return switch (self) {
            .transaction_timed_out, .network_error, .lock_conflict, .insufficient_space => true,
            else => false,
        };
    }

    /// Check if this represents a fatal error that requires abort
    pub fn isFatal(self: ZigErrorCode) bool {
        return switch (self) {
            .corrupted_data, .checksum_mismatch, .deadlock_detected, .out_of_memory => true,
            else => false,
        };
    }

    /// Get default severity for this error code
    pub fn defaultSeverity(self: ZigErrorCode) ErrorSeverity {
        return switch (self) {
            .success => .warning,
            .transaction_timed_out, .lock_conflict => .warning,
            .corrupted_data, .checksum_mismatch, .deadlock_detected, .out_of_memory => .critical,
            else => .err,
        };
    }
};

/// Error context entry for additional information
pub const ErrorContextEntry = struct {
    key: [64]u8,
    value: [256]u8,
    key_len: u8,
    value_len: u8,

    pub fn init(key: []const u8, value: []const u8) ErrorContextEntry {
        var entry = ErrorContextEntry{
            .key = [_]u8{0} ** 64,
            .value = [_]u8{0} ** 256,
            .key_len = @min(key.len, 63),
            .value_len = @min(value.len, 255),
        };

        @memcpy(entry.key[0..entry.key_len], key[0..entry.key_len]);
        @memcpy(entry.value[0..entry.value_len], value[0..entry.value_len]);

        return entry;
    }

    pub fn getKey(self: *const ErrorContextEntry) []const u8 {
        return self.key[0..self.key_len];
    }

    pub fn getValue(self: *const ErrorContextEntry) []const u8 {
        return self.value[0..self.value_len];
    }
};

/// Structured error information for cross-layer communication
pub const ZigError = struct {
    /// Error code
    code: ZigErrorCode,
    /// Error message
    message: [MAX_ERROR_MESSAGE_LEN]u8,
    message_len: u16,
    /// Error severity
    severity: ErrorSeverity,
    /// Shard ID where error occurred (if applicable)
    shard_id: ?u32,
    /// Transaction ID where error occurred (if applicable)
    transaction_id: ?u64,
    /// Additional context information
    context_entries: [MAX_ERROR_CONTEXT_ENTRIES]ErrorContextEntry,
    context_count: u8,
    /// Timestamp when error occurred
    timestamp_ns: u64,

    pub fn init(code: ZigErrorCode, message: []const u8) ZigError {
        var zig_error = ZigError{
            .code = code,
            .message = [_]u8{0} ** MAX_ERROR_MESSAGE_LEN,
            .message_len = @min(message.len, MAX_ERROR_MESSAGE_LEN - 1),
            .severity = code.defaultSeverity(),
            .shard_id = null,
            .transaction_id = null,
            .context_entries = [_]ErrorContextEntry{std.mem.zeroes(ErrorContextEntry)} ** MAX_ERROR_CONTEXT_ENTRIES,
            .context_count = 0,
            .timestamp_ns = @intCast(std.time.milliTimestamp()),
        };

        @memcpy(zig_error.message[0..zig_error.message_len], message[0..zig_error.message_len]);
        return zig_error;
    }

    pub fn withShard(self: ZigError, shard_id: u32) ZigError {
        var zig_error = self;
        zig_error.shard_id = shard_id;
        return zig_error;
    }

    pub fn withTransaction(self: ZigError, transaction_id: u64) ZigError {
        var zig_error = self;
        zig_error.transaction_id = transaction_id;
        return zig_error;
    }

    pub fn withSeverity(self: ZigError, severity: ErrorSeverity) ZigError {
        var zig_error = self;
        zig_error.severity = severity;
        return zig_error;
    }

    pub fn withContext(self: ZigError, key: []const u8, value: []const u8) ZigError {
        var zig_error = self;
        if (zig_error.context_count < MAX_ERROR_CONTEXT_ENTRIES) {
            zig_error.context_entries[zig_error.context_count] = ErrorContextEntry.init(key, value);
            zig_error.context_count += 1;
        }
        return zig_error;
    }

    pub fn getMessage(self: *const ZigError) []const u8 {
        return self.message[0..self.message_len];
    }

    /// Format error for logging
    pub fn format(self: *const ZigError, allocator: Allocator) ![]u8 {
        var parts = std.ArrayList([]const u8).init(allocator);
        defer parts.deinit();

        // Basic error info
        try parts.append(try std.fmt.allocPrint(allocator, "[{}] {}: {}", .{
            @tagName(self.severity),
            @tagName(self.code),
            self.getMessage(),
        }));

        // Add shard info if available
        if (self.shard_id) |shard_id| {
            try parts.append(try std.fmt.allocPrint(allocator, "shard={}", .{shard_id}));
        }

        // Add transaction info if available
        if (self.transaction_id) |txn_id| {
            try parts.append(try std.fmt.allocPrint(allocator, "txn={}", .{txn_id}));
        }

        // Add context entries
        for (0..self.context_count) |i| {
            const entry = &self.context_entries[i];
            try parts.append(try std.fmt.allocPrint(allocator, "{}={}", .{
                entry.getKey(),
                entry.getValue(),
            }));
        }

        return std.mem.join(allocator, " | ", parts.items);
    }
};

/// Thread-local error storage for each shard
const ShardErrorStorage = struct {
    last_error: ?ZigError,
    error_count: u64,
    mutex: std.Thread.Mutex,

    pub fn init() ShardErrorStorage {
        return ShardErrorStorage{
            .last_error = null,
            .error_count = 0,
            .mutex = .{},
        };
    }

    pub fn setError(self: *ShardErrorStorage, zig_error: ZigError) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        self.last_error = zig_error;
        self.error_count += 1;
    }

    pub fn getLastError(self: *ShardErrorStorage) ?ZigError {
        self.mutex.lock();
        defer self.mutex.unlock();

        return self.last_error;
    }

    pub fn clearError(self: *ShardErrorStorage) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        self.last_error = null;
    }

    pub fn getErrorCount(self: *const ShardErrorStorage) u64 {
        return self.error_count;
    }
};

/// Global error manager for cross-layer error handling
pub const CrossLayerErrorManager = struct {
    /// Error storage per shard
    shard_errors: std.AutoHashMap(u32, *ShardErrorStorage),
    /// Global allocator
    allocator: Allocator,
    /// Manager mutex
    mutex: std.Thread.Mutex,
    /// Global error statistics
    total_errors: u64,
    errors_by_code: std.AutoHashMap(ZigErrorCode, u64),

    pub fn init(allocator: Allocator) CrossLayerErrorManager {
        return CrossLayerErrorManager{
            .shard_errors = std.AutoHashMap(u32, *ShardErrorStorage).init(allocator),
            .allocator = allocator,
            .mutex = .{},
            .total_errors = 0,
            .errors_by_code = std.AutoHashMap(ZigErrorCode, u64).init(allocator),
        };
    }

    pub fn deinit(self: *CrossLayerErrorManager) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        var iter = self.shard_errors.valueIterator();
        while (iter.next()) |storage_ptr| {
            self.allocator.destroy(storage_ptr.*);
        }
        self.shard_errors.deinit();
        self.errors_by_code.deinit();
    }

    /// Get or create error storage for a shard
    fn getOrCreateShardStorage(self: *CrossLayerErrorManager, shard_id: u32) !*ShardErrorStorage {
        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.shard_errors.get(shard_id)) |storage| {
            return storage;
        }

        const storage = try self.allocator.create(ShardErrorStorage);
        storage.* = ShardErrorStorage.init();
        try self.shard_errors.put(shard_id, storage);
        return storage;
    }

    /// Record an error for a specific shard
    pub fn recordError(self: *CrossLayerErrorManager, shard_id: u32, zig_error: ZigError) !void {
        const storage = try self.getOrCreateShardStorage(shard_id);
        storage.setError(zig_error);

        // Update global statistics
        self.mutex.lock();
        defer self.mutex.unlock();

        self.total_errors += 1;
        const current_count = self.errors_by_code.get(zig_error.code) orelse 0;
        try self.errors_by_code.put(zig_error.code, current_count + 1);
    }

    /// Get the last error for a shard
    pub fn getLastError(self: *CrossLayerErrorManager, shard_id: u32) ?ZigError {
        const storage = self.getOrCreateShardStorage(shard_id) catch return null;
        return storage.getLastError();
    }

    /// Clear the last error for a shard
    pub fn clearError(self: *CrossLayerErrorManager, shard_id: u32) void {
        const storage = self.getOrCreateShardStorage(shard_id) catch return;
        storage.clearError();
    }

    /// Get error statistics
    pub fn getStatistics(self: *const CrossLayerErrorManager) struct {
        total_errors: u64,
        errors_by_code: std.AutoHashMap(ZigErrorCode, u64),
    } {
        return .{
            .total_errors = self.total_errors,
            .errors_by_code = self.errors_by_code,
        };
    }
};

/// Global error manager instance
var global_error_manager: ?CrossLayerErrorManager = null;
var global_error_manager_mutex: std.Thread.Mutex = .{};

/// Initialize global error manager
pub fn initGlobalErrorManager(allocator: Allocator) !void {
    global_error_manager_mutex.lock();
    defer global_error_manager_mutex.unlock();

    if (global_error_manager == null) {
        global_error_manager = CrossLayerErrorManager.init(allocator);
    }
}

/// Cleanup global error manager
pub fn deinitGlobalErrorManager() void {
    global_error_manager_mutex.lock();
    defer global_error_manager_mutex.unlock();

    if (global_error_manager) |*manager| {
        manager.deinit();
        global_error_manager = null;
    }
}

/// Get global error manager
fn getGlobalErrorManager() ?*CrossLayerErrorManager {
    global_error_manager_mutex.lock();
    defer global_error_manager_mutex.unlock();

    return if (global_error_manager) |*manager| manager else null;
}

/// Record an error globally
pub fn recordError(shard_id: u32, zig_error: ZigError) void {
    if (getGlobalErrorManager()) |manager| {
        manager.recordError(shard_id, zig_error) catch {};
    }
}

/// Get the last error for a shard
pub fn getLastError(shard_id: u32) ?ZigError {
    if (getGlobalErrorManager()) |manager| {
        return manager.getLastError(shard_id);
    }
    return null;
}

/// Clear the last error for a shard
pub fn clearError(shard_id: u32) void {
    if (getGlobalErrorManager()) |manager| {
        manager.clearError(shard_id);
    }
}

/// Recovery strategies for different error types
pub const ErrorRecoveryStrategy = struct {
    /// Determine recovery action for an error
    pub fn analyzeError(zig_error: *const ZigError) RecoveryAction {
        return switch (zig_error.code) {
            .success => .none,
            .transaction_timed_out => .{ .retry = .{ .max_attempts = 3, .backoff_ms = 1000 } },
            .lock_conflict => .{ .retry = .{ .max_attempts = 5, .backoff_ms = 100 } },
            .network_error => .{ .retry = .{ .max_attempts = 3, .backoff_ms = 2000 } },
            .insufficient_space => .cleanup,
            .deadlock_detected => .abort,
            .corrupted_data, .checksum_mismatch => .fatal,
            .out_of_memory => .fatal,
            else => .abort,
        };
    }
};

/// Recovery action recommendations
pub const RecoveryAction = union(enum) {
    /// No recovery needed
    none,
    /// Retry the operation
    retry: struct { max_attempts: u32, backoff_ms: u64 },
    /// Clean up resources and retry
    cleanup,
    /// Abort the current transaction
    abort,
    /// Fatal error - system needs attention
    fatal,
};

/// Convenience functions for creating common errors
pub fn outOfMemoryError(shard_id: ?u32) ZigError {
    var zig_error = ZigError.init(.out_of_memory, "Memory allocation failed");
    if (shard_id) |id| {
        zig_error = zig_error.withShard(id);
    }
    return zig_error.withSeverity(.critical);
}

pub fn transactionNotFoundError(shard_id: u32, transaction_id: u64) ZigError {
    return ZigError.init(.transaction_not_found, "Transaction not found")
        .withShard(shard_id)
        .withTransaction(transaction_id)
        .withContext("operation", "lookup");
}

pub fn walError(shard_id: u32, message: []const u8) ZigError {
    return ZigError.init(.wal_error, message)
        .withShard(shard_id)
        .withSeverity(.err)
        .withContext("component", "wal");
}

pub fn storageError(shard_id: u32, message: []const u8) ZigError {
    return ZigError.init(.storage_error, message)
        .withShard(shard_id)
        .withSeverity(.err)
        .withContext("component", "storage");
}

pub fn corruptedDataError(shard_id: u32, offset: u64) ZigError {
    var buffer: [32]u8 = undefined;
    const offset_str = std.fmt.bufPrint(&buffer, "{}", .{offset}) catch "unknown";

    return ZigError.init(.corrupted_data, "Data corruption detected")
        .withShard(shard_id)
        .withSeverity(.critical)
        .withContext("offset", offset_str)
        .withContext("component", "storage");
}

// ============================================================================
// FFI Functions for Rust Integration
// ============================================================================

/// Initialize error handling system
export fn zig_error_init() bool {
    initGlobalErrorManager(std.heap.c_allocator) catch return false;
    return true;
}

/// Cleanup error handling system
export fn zig_error_cleanup() void {
    deinitGlobalErrorManager();
}

/// Get last error for a shard
export fn zig_get_last_error(
    shard_id: u32,
    error_code: *c_int,
    message_buffer: [*]u8,
    buffer_size: usize,
) bool {
    const zig_error = getLastError(shard_id) orelse return false;

    error_code.* = zig_error.code.toInt();

    const message = zig_error.getMessage();
    const copy_len = @min(message.len, buffer_size - 1);
    for (0..copy_len) |i| {
        message_buffer[i] = message[i];
    }
    message_buffer[copy_len] = 0; // Null terminate

    return true;
}

/// Get extended error information for a shard
export fn zig_get_extended_error_info(
    shard_id: u32,
    error_code: *c_int,
    severity: *u8,
    shard_id_out: *u32,
    transaction_id_out: *u64,
    has_transaction: *bool,
    timestamp_ns: *u64,
    message_buffer: [*]u8,
    message_buffer_size: usize,
    context_keys: *[MAX_ERROR_CONTEXT_ENTRIES][64]u8,
    context_values: *[MAX_ERROR_CONTEXT_ENTRIES][256]u8,
    context_count: *u8,
) bool {
    const zig_error = getLastError(shard_id) orelse return false;

    error_code.* = zig_error.code.toInt();
    severity.* = @intFromEnum(zig_error.severity);
    shard_id_out.* = zig_error.shard_id orelse 0;

    if (zig_error.transaction_id) |txn_id| {
        transaction_id_out.* = txn_id;
        has_transaction.* = true;
    } else {
        transaction_id_out.* = 0;
        has_transaction.* = false;
    }

    timestamp_ns.* = zig_error.timestamp_ns;

    // Copy message
    const message = zig_error.getMessage();
    const copy_len = @min(message.len, message_buffer_size - 1);
    for (0..copy_len) |i| {
        message_buffer[i] = message[i];
    }
    message_buffer[copy_len] = 0;

    // Copy context entries
    context_count.* = zig_error.context_count;
    for (0..zig_error.context_count) |i| {
        const entry = &zig_error.context_entries[i];
        const key = entry.getKey();
        const value = entry.getValue();

        @memcpy(context_keys[i][0..key.len], key);
        context_keys[i][key.len] = 0;

        @memcpy(context_values[i][0..value.len], value);
        context_values[i][value.len] = 0;
    }

    return true;
}

/// Clear last error for a shard
export fn zig_clear_error(shard_id: u32) void {
    clearError(shard_id);
}

/// Record an error for a shard (for testing)
export fn zig_record_test_error(
    shard_id: u32,
    error_code: c_int,
    message: [*:0]const u8,
    severity: u8,
) void {
    const code = ZigErrorCode.fromInt(error_code);
    const message_slice = std.mem.span(message);
    const error_severity = @as(ErrorSeverity, @enumFromInt(severity));

    const zig_error = ZigError.init(code, message_slice).withShard(shard_id).withSeverity(error_severity);
    recordError(shard_id, zig_error);
}

// ============================================================================
// Tests
// ============================================================================

test "ZigErrorCode conversion" {
    const testing = std.testing;

    try testing.expectEqual(ZigErrorCode.success, ZigErrorCode.fromInt(0));
    try testing.expectEqual(ZigErrorCode.out_of_memory, ZigErrorCode.fromInt(-1));
    try testing.expectEqual(ZigErrorCode.unknown, ZigErrorCode.fromInt(-12345));

    try testing.expectEqual(@as(c_int, 0), ZigErrorCode.success.toInt());
    try testing.expectEqual(@as(c_int, -1), ZigErrorCode.out_of_memory.toInt());
}

test "ZigError creation and properties" {
    const testing = std.testing;

    const zig_error = ZigError.init(.transaction_not_found, "Transaction 123 not found")
        .withShard(0)
        .withTransaction(123)
        .withContext("operation", "commit");

    try testing.expectEqual(ZigErrorCode.transaction_not_found, zig_error.code);
    try testing.expectEqualStrings("Transaction 123 not found", zig_error.getMessage());
    try testing.expectEqual(@as(u32, 0), zig_error.shard_id.?);
    try testing.expectEqual(@as(u64, 123), zig_error.transaction_id.?);
    try testing.expectEqual(@as(u8, 1), zig_error.context_count);
}

test "CrossLayerErrorManager" {
    const testing = std.testing;

    var manager = CrossLayerErrorManager.init(testing.allocator);
    defer manager.deinit();

    const zig_error = ZigError.init(.out_of_memory, "Test error").withShard(0);
    try manager.recordError(0, zig_error);

    const retrieved = manager.getLastError(0);
    try testing.expect(retrieved != null);
    try testing.expectEqual(ZigErrorCode.out_of_memory, retrieved.?.code);

    manager.clearError(0);
    const cleared = manager.getLastError(0);
    try testing.expect(cleared == null);
}

test "Error recovery strategy" {
    const testing = std.testing;

    const timeout_error = ZigError.init(.transaction_timed_out, "Timeout");
    const recovery = ErrorRecoveryStrategy.analyzeError(&timeout_error);

    switch (recovery) {
        .retry => |retry_info| {
            try testing.expectEqual(@as(u32, 3), retry_info.max_attempts);
            try testing.expectEqual(@as(u64, 1000), retry_info.backoff_ms);
        },
        else => try testing.expect(false),
    }

    const corruption_error = ZigError.init(.corrupted_data, "Corrupted");
    const fatal_recovery = ErrorRecoveryStrategy.analyzeError(&corruption_error);
    try testing.expectEqual(RecoveryAction.fatal, fatal_recovery);
}

test "Global error manager" {
    const testing = std.testing;

    try initGlobalErrorManager(testing.allocator);
    defer deinitGlobalErrorManager();

    const zig_error = ZigError.init(.storage_error, "Test storage error").withShard(1);
    recordError(1, zig_error);

    const retrieved = getLastError(1);
    try testing.expect(retrieved != null);
    try testing.expectEqual(ZigErrorCode.storage_error, retrieved.?.code);

    clearError(1);
    const cleared = getLastError(1);
    try testing.expect(cleared == null);
}
