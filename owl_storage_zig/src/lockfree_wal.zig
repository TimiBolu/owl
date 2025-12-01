//! Lock-Free Write-Ahead Log (WAL) Implementation
//!
//! This module provides a lock-free WAL implementation that eliminates
//! mutex contention for high-throughput concurrent writes.
//!
//! Key Features:
//! - Lock-free LSN generation using atomic operations
//! - Lock-free append operations with atomic ordering
//! - Lock-free group commit coordination
//! - Atomic WAL entry commit without locks
//!
//! Performance Targets:
//! - 10M+ writes per second on 32-core machines
//! - Zero lock contention under concurrent load
//! - Predictable latency without lock-induced jitter

const std = @import("std");
const Allocator = std.mem.Allocator;
const File = std.fs.File;
const ObjRef = @import("objref.zig").ObjRef;

// Import types from regular WAL
const WAL_MAGIC = @import("wal.zig").WAL_MAGIC;
const WAL_VERSION = @import("wal.zig").WAL_VERSION;
const MAX_WAL_SIZE = @import("wal.zig").MAX_WAL_SIZE;
const WAL_ALIGNMENT = @import("wal.zig").WAL_ALIGNMENT;

pub const LSN = @import("wal.zig").LSN;
pub const EntryType = @import("wal.zig").EntryType;
pub const WALEntryHeader = @import("wal.zig").WALEntryHeader;
pub const WALEntry = @import("wal.zig").WALEntry;
pub const WALStats = @import("wal.zig").WALStats;
pub const RecoveryCallback = @import("wal.zig").RecoveryCallback;
pub const RecoveryOptions = @import("wal.zig").RecoveryOptions;
pub const GroupCommitConfig = @import("wal.zig").GroupCommitConfig;

/// Append operation state for lock-free coordination
const AppendOp = struct {
    entry: WALEntry,
    lsn: LSN,
    offset: u64,
    completed: std.atomic.Value(bool),
    next: ?*AppendOp,

    pub fn init(entry: WALEntry, lsn: LSN) AppendOp {
        return AppendOp{
            .entry = entry,
            .lsn = lsn,
            .offset = 0,
            .completed = std.atomic.Value(bool).init(false),
            .next = null,
        };
    }
};

/// Lock-free Write-Ahead Log manager
pub const LockFreeWAL = struct {
    allocator: Allocator,
    base_path: []const u8,
    current_file: ?File,

    // Lock-free LSN generation
    current_lsn: std.atomic.Value(LSN),

    // Lock-free file offset tracking
    current_offset: std.atomic.Value(u64),

    // Lock-free statistics
    entry_count: std.atomic.Value(u64),

    stats: WALStats,

    // Group commit support (lock-free)
    group_commit_config: GroupCommitConfig,

    // Lock-free pending operations queue
    pending_head: std.atomic.Value(?*AppendOp),
    pending_tail: std.atomic.Value(?*AppendOp),
    pending_count: std.atomic.Value(usize),

    last_flush_time: std.atomic.Value(i64),

    const Self = @This();

    /// Initialize lock-free WAL with base path
    pub fn init(allocator: Allocator, base_path: []const u8) !Self {
        return initWithConfig(allocator, base_path, GroupCommitConfig{});
    }

    /// Initialize lock-free WAL with custom group commit configuration
    pub fn initWithConfig(allocator: Allocator, base_path: []const u8, config: GroupCommitConfig) !Self {
        // Ensure WAL directory exists
        std.fs.cwd().makeDir(base_path) catch |err| switch (err) {
            error.PathAlreadyExists => {},
            else => return err,
        };

        var wal = Self{
            .allocator = allocator,
            .base_path = try allocator.dupe(u8, base_path),
            .current_file = null,
            .current_lsn = std.atomic.Value(LSN).init(1),
            .current_offset = std.atomic.Value(u64).init(0),
            .entry_count = std.atomic.Value(u64).init(0),
            .stats = WALStats.init(),
            .group_commit_config = config,
            .pending_head = std.atomic.Value(?*AppendOp).init(null),
            .pending_tail = std.atomic.Value(?*AppendOp).init(null),
            .pending_count = std.atomic.Value(usize).init(0),
            .last_flush_time = std.atomic.Value(i64).init(std.time.milliTimestamp()),
        };

        // Recover from existing WAL files
        try wal.recover();

        // Open current WAL file
        try wal.openCurrentFile();

        return wal;
    }

    /// Cleanup WAL resources
    pub fn deinit(self: *Self) void {
        // Flush any pending entries
        self.flushPending() catch |err| {
            std.log.err("Failed to flush pending entries during deinit: {}", .{err});
        };

        // Clean up pending operations
        var current = self.pending_head.load(.acquire);
        while (current) |op| {
            const next = op.next;
            op.entry.deinit(self.allocator);
            self.allocator.destroy(op);
            current = next;
        }

        if (self.current_file) |file| {
            file.close();
        }
        self.allocator.free(self.base_path);
    }

    /// Get next LSN atomically (lock-free)
    pub fn nextLSN(self: *Self) LSN {
        return self.current_lsn.fetchAdd(1, .seq_cst);
    }

    /// Get current LSN (lock-free)
    pub fn getCurrentLSN(self: *Self) LSN {
        return self.current_lsn.load(.seq_cst);
    }

    /// Atomically reserve space in WAL file (lock-free)
    fn reserveSpace(self: *Self, size: u64) u64 {
        return self.current_offset.fetchAdd(size, .seq_cst);
    }

    /// Append entry to WAL with lock-free atomic operations
    pub fn append(self: *Self, entry: *const WALEntry) !void {
        const file = self.current_file orelse return error.WALNotOpen;

        // Check if we need to rotate the file
        const current_size = self.current_offset.load(.acquire);
        if (current_size + entry.alignedSize() > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        // Atomically reserve space in the file
        const offset = self.reserveSpace(entry.alignedSize());

        // Write entry at reserved offset
        try file.seekTo(offset);

        // Write entry header
        const header_bytes = std.mem.asBytes(&entry.header);
        _ = try file.writeAll(header_bytes);

        // Write entry data
        _ = try file.writeAll(entry.data);

        // Write padding for alignment
        const padding_size = entry.alignedSize() - entry.totalSize();
        if (padding_size > 0) {
            const padding = [_]u8{0} ** WAL_ALIGNMENT;
            _ = try file.writeAll(padding[0..padding_size]);
        }

        // Atomic sync to disk (durability guarantee)
        try file.sync();

        // Update statistics atomically
        _ = self.entry_count.fetchAdd(1, .monotonic);

        // Update last LSN (may race, but that's okay - we just need a recent value)
        self.stats.last_lsn = entry.header.lsn;
    }

    /// Atomically append entry with explicit fsync for durability (lock-free)
    pub fn atomicAppend(self: *Self, entry: *const WALEntry) !void {
        return self.append(entry);
    }

    /// Atomically append entry with explicit fsync (lock-free)
    pub fn atomicAppendWithSync(self: *Self, entry: *const WALEntry) !void {
        return self.append(entry);
    }

    /// Append entry with lock-free group commit (buffered, higher throughput)
    pub fn appendBuffered(self: *Self, entry: WALEntry) !void {
        // Create append operation
        const op = try self.allocator.create(AppendOp);
        op.* = AppendOp.init(entry, entry.header.lsn);

        // Add to pending queue using lock-free algorithm
        while (true) {
            const current_tail = self.pending_tail.load(.acquire);

            if (current_tail == null) {
                // Queue is empty, try to set both head and tail
                if (self.pending_head.cmpxchgWeak(
                    null,
                    op,
                    .release,
                    .acquire,
                )) |_| {
                    continue;
                }

                _ = self.pending_tail.cmpxchgStrong(
                    null,
                    op,
                    .release,
                    .acquire,
                );

                _ = self.pending_count.fetchAdd(1, .monotonic);
                break;
            } else {
                // Queue has items, append to tail
                op.next = null;

                // Try to update tail's next pointer
                const tail_ptr = current_tail.?;
                if (@cmpxchgWeak(
                    ?*AppendOp,
                    &tail_ptr.next,
                    null,
                    op,
                    .release,
                    .acquire,
                )) |_| {
                    continue;
                }

                // Update tail pointer
                _ = self.pending_tail.cmpxchgStrong(
                    current_tail,
                    op,
                    .release,
                    .acquire,
                );

                _ = self.pending_count.fetchAdd(1, .monotonic);
                break;
            }
        }

        // Check if we should flush (lock-free check)
        const count = self.pending_count.load(.monotonic);
        const now = std.time.milliTimestamp();
        const last_flush = self.last_flush_time.load(.monotonic);

        const should_flush =
            count >= self.group_commit_config.max_entries or
            (now - last_flush) >= self.group_commit_config.max_wait_ms;

        if (should_flush) {
            try self.flushPending();
        }
    }

    /// Force flush of pending entries (lock-free)
    pub fn flush(self: *Self) !void {
        try self.flushPending();
    }

    /// Flush pending entries using lock-free algorithm
    fn flushPending(self: *Self) !void {
        // Atomically swap out the pending queue
        const head = self.pending_head.swap(null, .acq_rel);
        _ = self.pending_tail.swap(null, .acq_rel);
        const count = self.pending_count.swap(0, .acq_rel);

        if (head == null or count == 0) return;

        const file = self.current_file orelse return error.WALNotOpen;

        // Calculate total size needed
        var total_size: u64 = 0;
        var current = head;
        while (current) |op| {
            total_size += op.entry.alignedSize();
            current = op.next;
        }

        // Check if we need to rotate
        const current_offset = self.current_offset.load(.acquire);
        if (current_offset + total_size > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        // Atomically reserve space for all entries
        const base_offset = self.reserveSpace(total_size);

        // Write all entries at their reserved offsets
        var offset = base_offset;
        current = head;

        while (current) |op| {
            try file.seekTo(offset);

            // Write header
            const header_bytes = std.mem.asBytes(&op.entry.header);
            _ = try file.writeAll(header_bytes);

            // Write data
            _ = try file.writeAll(op.entry.data);

            // Write padding
            const padding_size = op.entry.alignedSize() - op.entry.totalSize();
            if (padding_size > 0) {
                const padding = [_]u8{0} ** WAL_ALIGNMENT;
                _ = try file.writeAll(padding[0..padding_size]);
            }

            // Update stats
            _ = self.entry_count.fetchAdd(1, .monotonic);
            self.stats.last_lsn = op.entry.header.lsn;

            offset += op.entry.alignedSize();
            current = op.next;
        }

        // Single sync for all entries (group commit!)
        try file.sync();

        // Update last flush time
        _ = self.last_flush_time.store(std.time.milliTimestamp(), .release);

        // Clean up operations
        current = head;
        while (current) |op| {
            const next = op.next;
            op.entry.deinit(self.allocator);
            self.allocator.destroy(op);
            current = next;
        }
    }

    /// Append multiple entries atomically (lock-free group commit)
    pub fn appendBatch(self: *Self, entries: []const WALEntry) !void {
        if (entries.len == 0) return;

        const file = self.current_file orelse return error.WALNotOpen;

        // Calculate total size needed
        var total_size: u64 = 0;
        for (entries) |*entry| {
            total_size += entry.alignedSize();
        }

        // Check if we need to rotate
        const current_offset = self.current_offset.load(.acquire);
        if (current_offset + total_size > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        // Atomically reserve space for all entries
        const base_offset = self.reserveSpace(total_size);

        // Write all entries
        var offset = base_offset;
        for (entries) |*entry| {
            try file.seekTo(offset);

            // Write header
            const header_bytes = std.mem.asBytes(&entry.header);
            _ = try file.writeAll(header_bytes);

            // Write data
            _ = try file.writeAll(entry.data);

            // Write padding
            const padding_size = entry.alignedSize() - entry.totalSize();
            if (padding_size > 0) {
                const padding = [_]u8{0} ** WAL_ALIGNMENT;
                _ = try file.writeAll(padding[0..padding_size]);
            }

            // Update stats
            _ = self.entry_count.fetchAdd(1, .monotonic);
            self.stats.last_lsn = entry.header.lsn;

            offset += entry.alignedSize();
        }

        // Single sync for all entries (group commit)
        try file.sync();
    }

    /// Create checkpoint entry (lock-free)
    pub fn checkpoint(self: *Self) !LSN {
        const lsn = self.nextLSN();
        const checkpoint_data = "CHECKPOINT";

        var entry = try WALEntry.init(
            self.allocator,
            lsn,
            .checkpoint,
            ObjRef.init(0),
            checkpoint_data,
        );
        defer entry.deinit(self.allocator);

        try self.append(&entry);

        self.stats.last_checkpoint = lsn;
        return lsn;
    }

    /// Get WAL statistics
    pub fn getStats(self: *const Self) WALStats {
        var stats = self.stats;
        stats.entry_count = self.entry_count.load(.monotonic);
        return stats;
    }

    /// Recover from existing WAL files
    fn recover(self: *Self) !void {
        // Use the regular WAL recovery logic
        const RegularWAL = @import("wal.zig").WAL;

        var temp_wal = try RegularWAL.init(self.allocator, self.base_path);
        defer temp_wal.deinit();

        const stats = temp_wal.getStats();

        // Copy recovered state
        self.current_lsn.store(temp_wal.getCurrentLSN(), .release);
        self.stats = stats;
        self.entry_count.store(stats.entry_count, .release);
    }

    /// Open current WAL file for writing
    fn openCurrentFile(self: *Self) !void {
        const filename = try std.fmt.allocPrint(self.allocator, "wal_{:08}.wal", .{self.getCurrentLSN()});
        defer self.allocator.free(filename);

        const file_path = try std.fs.path.join(self.allocator, &[_][]const u8{ self.base_path, filename });
        defer self.allocator.free(file_path);

        // Open file for writing
        self.current_file = try std.fs.cwd().createFile(file_path, .{ .truncate = false });

        // Get current file size
        const stat = try self.current_file.?.stat();
        self.current_offset.store(stat.size, .release);

        // If file is empty, write header
        if (stat.size == 0) {
            const WALHeader = @import("wal.zig").WALHeader;
            var header = WALHeader.init();
            header.updateChecksum();

            const header_bytes = std.mem.asBytes(&header);
            _ = try self.current_file.?.writeAll(header_bytes);

            self.current_offset.store(@sizeOf(WALHeader), .release);
        }

        // Seek to end of file
        try self.current_file.?.seekTo(self.current_offset.load(.acquire));
    }

    /// Rotate to new WAL file
    fn rotateFile(self: *Self) !void {
        // Close current file
        if (self.current_file) |file| {
            file.close();
            self.current_file = null;
        }

        // Open new file
        try self.openCurrentFile();

        self.stats.file_count += 1;

        std.log.info("Lock-free WAL file rotated, new file count: {}", .{self.stats.file_count});
    }
};

// Tests
test "LockFreeWAL: basic operations" {
    const allocator = std.testing.allocator;
    const test_dir = "test_lockfree_wal_basic";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try LockFreeWAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create test entry
    const data = "test insert data";
    const objref = ObjRef.init(2048);

    var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
    defer entry.deinit(allocator);

    // Append entry
    try wal.append(&entry);

    // Check stats
    const stats = wal.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.entry_count);
    try std.testing.expectEqual(@as(LSN, 1), stats.last_lsn);
}

test "LockFreeWAL: atomic LSN generation" {
    const allocator = std.testing.allocator;
    const test_dir = "test_lockfree_wal_lsn";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try LockFreeWAL.init(allocator, test_dir);
    defer wal.deinit();

    // Test that LSN generation is atomic and sequential
    const lsn1 = wal.nextLSN();
    const lsn2 = wal.nextLSN();
    const lsn3 = wal.nextLSN();

    try std.testing.expectEqual(@as(LSN, 1), lsn1);
    try std.testing.expectEqual(@as(LSN, 2), lsn2);
    try std.testing.expectEqual(@as(LSN, 3), lsn3);
}

test "LockFreeWAL: buffered append" {
    const allocator = std.testing.allocator;
    const test_dir = "test_lockfree_wal_buffered";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try LockFreeWAL.init(allocator, test_dir);
    defer wal.deinit();

    // Append multiple entries with buffering
    var i: usize = 0;
    while (i < 10) : (i += 1) {
        const data = "buffered entry";
        const objref = ObjRef.init(1024 + i * 100);
        const entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
        try wal.appendBuffered(entry);
    }

    // Force flush
    try wal.flush();

    // Check stats
    const stats = wal.getStats();
    try std.testing.expectEqual(@as(u64, 10), stats.entry_count);
}

test "LockFreeWAL: batch append" {
    const allocator = std.testing.allocator;
    const test_dir = "test_lockfree_wal_batch";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try LockFreeWAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create batch of entries
    var entries: [5]WALEntry = undefined;
    var i: usize = 0;
    while (i < 5) : (i += 1) {
        const data = "batch entry";
        const objref = ObjRef.init(3000 + i * 100);
        entries[i] = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
    }
    defer {
        for (&entries) |*entry| {
            entry.deinit(allocator);
        }
    }

    // Append batch
    try wal.appendBatch(&entries);

    // Check stats
    const stats = wal.getStats();
    try std.testing.expectEqual(@as(u64, 5), stats.entry_count);
}

test "LockFreeWAL: checkpoint" {
    const allocator = std.testing.allocator;
    const test_dir = "test_lockfree_wal_checkpoint";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try LockFreeWAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create checkpoint
    const checkpoint_lsn = try wal.checkpoint();

    // Check stats
    const stats = wal.getStats();
    try std.testing.expectEqual(checkpoint_lsn, stats.last_checkpoint);
    try std.testing.expectEqual(@as(u64, 1), stats.entry_count);
}
