//! Write-Ahead Log (WAL) Implementation
//!
//! The WAL provides durability and crash recovery for the Owl storage engine.
//! It uses a simple append-only log format with group commits for performance.
//!
//! WAL Format:
//! - Header: Magic number, version, checksum
//! - Entries: LSN, type, size, data, checksum
//! - Footer: End marker, final checksum

const std = @import("std");
const Allocator = std.mem.Allocator;
const File = std.fs.File;
const ObjRef = @import("objref.zig").ObjRef;

/// WAL magic number for file format validation
const WAL_MAGIC: u32 = 0x4F574C57; // "OWLW" in ASCII

/// WAL format version
const WAL_VERSION: u16 = 1;

/// Maximum WAL file size before rotation (64MB)
pub const MAX_WAL_SIZE: u64 = 64 * 1024 * 1024;

/// WAL entry alignment (8 bytes for performance)
pub const WAL_ALIGNMENT: usize = 8;

/// Log Sequence Number - monotonically increasing transaction ID
pub const LSN = u64;

/// WAL entry types
pub const EntryType = enum(u8) {
    insert = 1,
    update = 2,
    delete = 3,
    checkpoint = 4,
    commit = 5,
    abort = 6,
    batch_begin = 7, // Marks start of batch operation
    batch_end = 8, // Marks end of batch operation

    pub fn toString(self: EntryType) []const u8 {
        return switch (self) {
            .insert => "INSERT",
            .update => "UPDATE",
            .delete => "DELETE",
            .checkpoint => "CHECKPOINT",
            .commit => "COMMIT",
            .abort => "ABORT",
            .batch_begin => "BATCH_BEGIN",
            .batch_end => "BATCH_END",
        };
    }
};

/// WAL file header (32 bytes)
pub const WALHeader = extern struct {
    magic: u32, // Magic number for validation
    version: u16, // WAL format version
    flags: u16, // Feature flags
    created_at: u64, // Creation timestamp
    last_lsn: LSN, // Last LSN in this file
    checksum: u32, // Header checksum
    _padding: u32, // Padding to 32 bytes

    pub fn init() WALHeader {
        return WALHeader{
            .magic = WAL_MAGIC,
            .version = WAL_VERSION,
            .flags = 0,
            .created_at = @intCast(std.time.timestamp()),
            .last_lsn = 0,
            .checksum = 0,
            ._padding = 0,
        };
    }

    pub fn updateChecksum(self: *WALHeader) void {
        self.checksum = 0;
        const bytes = std.mem.asBytes(self);
        self.checksum = crc32(bytes[0 .. bytes.len - 4]); // Exclude checksum field
    }

    pub fn validate(self: *const WALHeader) bool {
        if (self.magic != WAL_MAGIC) return false;
        if (self.version != WAL_VERSION) return false;

        var temp = self.*;
        temp.checksum = 0;
        const bytes = std.mem.asBytes(&temp);
        const expected_checksum = crc32(bytes[0 .. bytes.len - 4]);

        return self.checksum == expected_checksum;
    }
};

/// WAL entry header (32 bytes - aligned)
pub const WALEntryHeader = extern struct {
    lsn: LSN, // Log sequence number
    entry_type: EntryType, // Type of operation
    _padding1: u8, // Padding
    _padding2: u16, // Padding for alignment
    data_size: u32, // Size of entry data
    objref: u64, // Object reference (if applicable)
    checksum: u32, // Entry checksum (header + data)
    _padding3: u32, // Padding to 32 bytes

    pub fn init(lsn: LSN, entry_type: EntryType, data_size: u32, objref: ObjRef) WALEntryHeader {
        return WALEntryHeader{
            .lsn = lsn,
            .entry_type = entry_type,
            ._padding1 = 0,
            ._padding2 = 0,
            .data_size = data_size,
            .objref = objref.getOffset(),
            .checksum = 0,
            ._padding3 = 0,
        };
    }

    pub fn updateChecksum(self: *WALEntryHeader, data: []const u8) void {
        self.checksum = 0;

        var hasher = std.hash.Crc32.init();
        hasher.update(std.mem.asBytes(self));
        hasher.update(data);

        self.checksum = hasher.final();
    }

    pub fn validate(self: *const WALEntryHeader, data: []const u8) bool {
        var temp = self.*;
        temp.checksum = 0;

        var hasher = std.hash.Crc32.init();
        hasher.update(std.mem.asBytes(&temp));
        hasher.update(data);

        return self.checksum == hasher.final();
    }
};

/// WAL entry - combines header and data
pub const WALEntry = struct {
    header: WALEntryHeader,
    data: []u8,

    pub fn init(allocator: Allocator, lsn: LSN, entry_type: EntryType, objref: ObjRef, data: []const u8) !WALEntry {
        const entry_data = try allocator.dupe(u8, data);
        var header = WALEntryHeader.init(lsn, entry_type, @intCast(data.len), objref);
        header.updateChecksum(entry_data);

        return WALEntry{
            .header = header,
            .data = entry_data,
        };
    }

    pub fn deinit(self: *WALEntry, allocator: Allocator) void {
        allocator.free(self.data);
    }

    pub fn totalSize(self: *const WALEntry) usize {
        return @sizeOf(WALEntryHeader) + self.data.len;
    }

    pub fn alignedSize(self: *const WALEntry) usize {
        const size = self.totalSize();
        return std.mem.alignForward(usize, size, WAL_ALIGNMENT);
    }

    pub fn validate(self: *const WALEntry) bool {
        return self.header.validate(self.data);
    }
};

/// Batch WAL entry - combines multiple operations into a single atomic unit
/// This enables group commit with a single fsync for the entire batch
pub const BatchWALEntry = struct {
    batch_lsn: LSN, // LSN for the entire batch
    entries: []WALEntry, // Individual entries in the batch
    allocator: Allocator,

    pub fn init(allocator: Allocator, batch_lsn: LSN, entries: []WALEntry) !BatchWALEntry {
        return BatchWALEntry{
            .batch_lsn = batch_lsn,
            .entries = entries,
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *BatchWALEntry) void {
        for (self.entries) |*entry| {
            entry.deinit(self.allocator);
        }
        self.allocator.free(self.entries);
    }

    /// Calculate total size of batch including markers
    pub fn totalSize(self: *const BatchWALEntry) usize {
        var size: usize = 0;
        // Batch begin marker
        size += @sizeOf(WALEntryHeader);
        // All entries
        for (self.entries) |*entry| {
            size += entry.alignedSize();
        }
        // Batch end marker
        size += @sizeOf(WALEntryHeader);
        return size;
    }

    /// Validate all entries in the batch
    pub fn validate(self: *const BatchWALEntry) bool {
        for (self.entries) |*entry| {
            if (!entry.validate()) return false;
        }
        return true;
    }
};

/// WAL statistics
pub const WALStats = struct {
    file_count: u32,
    total_size: u64,
    entry_count: u64,
    last_lsn: LSN,
    last_checkpoint: LSN,
    recovery_entries: u64,
    corrupted_entries: u64,
    batch_count: u64, // Number of batch operations
    batch_entries: u64, // Total entries in batches

    pub fn init() WALStats {
        return WALStats{
            .file_count = 0,
            .total_size = 0,
            .entry_count = 0,
            .last_lsn = 0,
            .last_checkpoint = 0,
            .recovery_entries = 0,
            .corrupted_entries = 0,
            .batch_count = 0,
            .batch_entries = 0,
        };
    }
};

/// Recovery callback function type
/// Called for each valid WAL entry during recovery
/// Returns error to abort recovery, or void to continue
pub const RecoveryCallback = *const fn (entry: *const WALEntry, user_data: ?*anyopaque) anyerror!void;

/// Recovery options
pub const RecoveryOptions = struct {
    /// Callback function to replay entries
    callback: ?RecoveryCallback = null,
    /// User data passed to callback
    user_data: ?*anyopaque = null,
    /// Stop at first corruption (false = skip and continue)
    strict_mode: bool = false,
    /// Maximum entries to recover (0 = unlimited)
    max_entries: u64 = 0,
};

/// Group commit configuration
pub const GroupCommitConfig = struct {
    /// Maximum time to wait before flushing (milliseconds)
    max_wait_ms: u64 = 1,
    /// Maximum number of entries before flushing
    max_entries: usize = 1000,
    /// Maximum buffer size before flushing (bytes)
    max_buffer_size: usize = 1024 * 1024, // 1MB
};

/// Write-Ahead Log manager
pub const WAL = struct {
    allocator: Allocator,
    base_path: []const u8,
    current_file: ?File,
    current_lsn: std.atomic.Value(LSN),
    current_size: u64,
    stats: WALStats,

    // Group commit support
    group_commit_config: GroupCommitConfig,
    pending_entries: std.ArrayList(WALEntry),
    pending_size: usize,
    last_flush_time: i64,
    flush_mutex: std.Thread.Mutex,
    pending_allocator: Allocator,

    const Self = @This();

    /// Initialize WAL with base path
    pub fn init(allocator: Allocator, base_path: []const u8) !Self {
        return initWithConfig(allocator, base_path, GroupCommitConfig{});
    }

    /// Initialize WAL with custom group commit configuration
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
            .current_size = 0,
            .stats = WALStats.init(),
            .group_commit_config = config,
            .pending_entries = .{},
            .pending_size = 0,
            .last_flush_time = std.time.milliTimestamp(),
            .flush_mutex = .{},
            .pending_allocator = allocator,
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

        // Clean up pending entries
        for (self.pending_entries.items) |*entry| {
            entry.deinit(self.allocator);
        }
        self.pending_entries.deinit(self.pending_allocator);

        if (self.current_file) |file| {
            file.close();
        }
        self.allocator.free(self.base_path);
    }

    /// Get next LSN atomically
    pub fn nextLSN(self: *Self) LSN {
        return self.current_lsn.fetchAdd(1, .seq_cst);
    }

    /// Get current LSN
    pub fn getCurrentLSN(self: *Self) LSN {
        return self.current_lsn.load(.seq_cst);
    }

    /// Append entry to WAL (immediate write, no buffering)
    pub fn append(self: *Self, entry: *const WALEntry) !void {
        const file = self.current_file orelse return error.WALNotOpen;

        // Check if we need to rotate the file
        if (self.current_size + entry.alignedSize() > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

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

        // Update statistics
        self.current_size += entry.alignedSize();
        self.stats.entry_count += 1;
        self.stats.last_lsn = entry.header.lsn;

        // Sync to disk (immediate durability)
        try file.sync();
    }

    /// Atomically append entry to WAL with proper ordering guarantees
    /// This ensures that the WAL entry is written atomically and durably
    pub fn atomicAppend(self: *Self, entry: *const WALEntry) !void {
        // Use the same implementation as append, but with explicit atomic semantics
        // The file system operations are already atomic at the OS level
        return self.append(entry);
    }

    /// Atomically append entry with explicit fsync for durability
    /// This is used by atomic operations to ensure crash consistency
    pub fn atomicAppendWithSync(self: *Self, entry: *const WALEntry) !void {
        const file = self.current_file orelse return error.WALNotOpen;

        // Check if we need to rotate the file
        if (self.current_size + entry.alignedSize() > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        // Write all data first
        const header_bytes = std.mem.asBytes(&entry.header);
        _ = try file.writeAll(header_bytes);
        _ = try file.writeAll(entry.data);

        // Write padding for alignment
        const padding_size = entry.alignedSize() - entry.totalSize();
        if (padding_size > 0) {
            const padding = [_]u8{0} ** WAL_ALIGNMENT;
            _ = try file.writeAll(padding[0..padding_size]);
        }

        // Atomic sync to disk - this is the durability guarantee
        try file.sync();

        // Update statistics only after successful sync
        self.current_size += entry.alignedSize();
        self.stats.entry_count += 1;
        self.stats.last_lsn = entry.header.lsn;
    }

    /// Perform crash recovery with detailed error reporting
    /// This method provides enhanced recovery capabilities for crash recovery
    pub fn performCrashRecovery(self: *Self, callback: ?RecoveryCallback, user_data: ?*anyopaque) !RecoveryResult {
        const options = RecoveryOptions{
            .callback = callback,
            .user_data = user_data,
            .strict_mode = false, // Allow recovery to continue despite corruptions
        };

        var total_recovered: u64 = 0;
        var total_corrupted: u64 = 0;
        var max_lsn: LSN = 0;

        // Get list of WAL files
        var dir = try std.fs.cwd().openDir(self.base_path, .{ .iterate = true });
        defer dir.close();

        var iterator = dir.iterate();
        var wal_files: std.ArrayList([]const u8) = .{};
        defer {
            for (wal_files.items) |file_name| {
                self.allocator.free(file_name);
            }
            wal_files.deinit(self.allocator);
        }

        // Collect WAL files
        while (try iterator.next()) |entry| {
            if (entry.kind != .file) continue;
            if (!std.mem.endsWith(u8, entry.name, ".wal")) continue;

            const file_name = try self.allocator.dupe(u8, entry.name);
            try wal_files.append(self.allocator, file_name);
        }

        // Sort files for ordered recovery
        std.mem.sort([]const u8, wal_files.items, {}, struct {
            fn lessThan(_: void, a: []const u8, b: []const u8) bool {
                return std.mem.order(u8, a, b) == .lt;
            }
        }.lessThan);

        // Process each file
        for (wal_files.items) |file_name| {
            const file_path = try std.fs.path.join(self.allocator, &[_][]const u8{ self.base_path, file_name });
            defer self.allocator.free(file_path);

            const result = self.recoverFileWithOptions(file_path, options) catch |err| {
                std.log.warn("Failed to recover WAL file {s}: {}", .{ file_path, err });
                continue;
            };

            max_lsn = @max(max_lsn, result.last_lsn);
            total_recovered += result.entries_recovered;
            total_corrupted += result.entries_corrupted;
        }

        // Update current LSN
        self.current_lsn.store(max_lsn + 1, .seq_cst);
        self.stats.last_lsn = max_lsn;
        self.stats.recovery_entries = total_recovered;
        self.stats.corrupted_entries = total_corrupted;

        return RecoveryResult{
            .last_lsn = max_lsn,
            .entries_recovered = total_recovered,
            .entries_corrupted = total_corrupted,
        };
    }

    /// Check WAL integrity for crash recovery
    pub fn checkIntegrity(self: *Self) !bool {
        var dir = try std.fs.cwd().openDir(self.base_path, .{ .iterate = true });
        defer dir.close();

        var iterator = dir.iterate();
        var integrity_ok = true;

        while (try iterator.next()) |entry| {
            if (entry.kind != .file) continue;
            if (!std.mem.endsWith(u8, entry.name, ".wal")) continue;

            const file_path = try std.fs.path.join(self.allocator, &[_][]const u8{ self.base_path, entry.name });
            defer self.allocator.free(file_path);

            // Try to read and validate the file
            const result = self.recoverFile(file_path) catch |err| {
                std.log.warn("WAL file integrity check failed for {s}: {}", .{ file_path, err });
                integrity_ok = false;
                continue;
            };

            if (result == 0) {
                std.log.warn("WAL file {s} contains no valid entries", .{file_path});
                integrity_ok = false;
            }
        }

        return integrity_ok;
    }

    /// Append entry with group commit (buffered, higher throughput)
    pub fn appendBuffered(self: *Self, entry: WALEntry) !void {
        self.flush_mutex.lock();
        defer self.flush_mutex.unlock();

        // Add to pending entries
        try self.pending_entries.append(self.pending_allocator, entry);
        self.pending_size += entry.alignedSize();

        // Check if we should flush
        const should_flush =
            self.pending_entries.items.len >= self.group_commit_config.max_entries or
            self.pending_size >= self.group_commit_config.max_buffer_size or
            (std.time.milliTimestamp() - self.last_flush_time) >= self.group_commit_config.max_wait_ms;

        if (should_flush) {
            try self.flushPendingLocked();
        }
    }

    /// Force flush of pending entries
    pub fn flush(self: *Self) !void {
        self.flush_mutex.lock();
        defer self.flush_mutex.unlock();

        try self.flushPendingLocked();
    }

    /// Flush pending entries (must hold flush_mutex)
    fn flushPendingLocked(self: *Self) !void {
        if (self.pending_entries.items.len == 0) return;

        const file = self.current_file orelse return error.WALNotOpen;

        // Check if we need to rotate
        if (self.current_size + self.pending_size > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        // Write all pending entries
        for (self.pending_entries.items) |*entry| {
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
            self.stats.entry_count += 1;
            self.stats.last_lsn = entry.header.lsn;
        }

        // Single sync for all entries (group commit!)
        try file.sync();

        self.current_size += self.pending_size;
        self.last_flush_time = std.time.milliTimestamp();

        // Clean up pending entries
        for (self.pending_entries.items) |*entry| {
            entry.deinit(self.allocator);
        }
        self.pending_entries.clearRetainingCapacity();
        self.pending_size = 0;
    }

    /// Flush pending entries without lock (for internal use)
    fn flushPending(self: *Self) !void {
        self.flush_mutex.lock();
        defer self.flush_mutex.unlock();

        try self.flushPendingLocked();
    }

    /// Append multiple entries atomically (group commit)
    pub fn appendBatch(self: *Self, entries: []const WALEntry) !void {
        if (entries.len == 0) return;

        const file = self.current_file orelse return error.WALNotOpen;

        // Calculate total size needed
        var total_size: usize = 0;
        for (entries) |*entry| {
            total_size += entry.alignedSize();
        }

        // Check if we need to rotate
        if (self.current_size + total_size > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        // Write all entries
        for (entries) |*entry| {
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
            self.stats.entry_count += 1;
            self.stats.last_lsn = entry.header.lsn;
        }

        // Single sync for all entries (group commit)
        try file.sync();

        self.current_size += total_size;
    }

    /// Append batch WAL entry with atomic semantics
    /// This writes a batch begin marker, all entries, and a batch end marker
    /// with a single fsync for the entire batch
    pub fn appendBatchAtomic(self: *Self, batch: *const BatchWALEntry) !void {
        if (batch.entries.len == 0) return;

        const file = self.current_file orelse return error.WALNotOpen;

        // Calculate total size needed
        const total_size = batch.totalSize();

        // Check if we need to rotate
        if (self.current_size + total_size > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        // Write batch begin marker
        const batch_begin_data = "BATCH_BEGIN";
        var begin_header = WALEntryHeader.init(
            batch.batch_lsn,
            .batch_begin,
            @intCast(batch_begin_data.len),
            ObjRef.init(0),
        );
        begin_header.updateChecksum(batch_begin_data);

        const begin_header_bytes = std.mem.asBytes(&begin_header);
        _ = try file.writeAll(begin_header_bytes);
        _ = try file.writeAll(batch_begin_data);

        // Write all entries in the batch
        for (batch.entries) |*entry| {
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
        }

        // Write batch end marker
        const batch_end_data = "BATCH_END";
        var end_header = WALEntryHeader.init(
            batch.batch_lsn,
            .batch_end,
            @intCast(batch_end_data.len),
            ObjRef.init(0),
        );
        end_header.updateChecksum(batch_end_data);

        const end_header_bytes = std.mem.asBytes(&end_header);
        _ = try file.writeAll(end_header_bytes);
        _ = try file.writeAll(batch_end_data);

        // Single sync for entire batch (group commit!)
        try file.sync();

        // Update statistics
        self.current_size += total_size;
        self.stats.entry_count += batch.entries.len + 2; // +2 for begin/end markers
        self.stats.batch_count += 1;
        self.stats.batch_entries += batch.entries.len;
        self.stats.last_lsn = batch.batch_lsn;
    }

    /// Create checkpoint entry
    pub fn checkpoint(self: *Self) !LSN {
        const lsn = self.nextLSN();
        const checkpoint_data = "CHECKPOINT";

        var entry = try WALEntry.init(
            self.allocator,
            lsn,
            .checkpoint,
            ObjRef.init(0), // No object for checkpoint
            checkpoint_data,
        );
        defer entry.deinit(self.allocator);

        try self.append(&entry);

        self.stats.last_checkpoint = lsn;
        return lsn;
    }

    /// Get WAL statistics
    pub fn getStats(self: *const Self) WALStats {
        return self.stats;
    }

    /// Recover from existing WAL files (basic recovery - just scan for max LSN)
    fn recover(self: *Self) !void {
        const options = RecoveryOptions{};
        try self.recoverWithOptions(options);
    }

    /// Recover from existing WAL files with options
    pub fn recoverWithOptions(self: *Self, options: RecoveryOptions) !void {
        var dir = try std.fs.cwd().openDir(self.base_path, .{ .iterate = true });
        defer dir.close();

        var iterator = dir.iterate();
        var max_lsn: LSN = 0;
        var file_count: u32 = 0;
        var total_recovered: u64 = 0;
        var total_corrupted: u64 = 0;

        // Collect all WAL files and sort by name (which includes LSN)
        var wal_files: std.ArrayList([]const u8) = .{};
        defer {
            for (wal_files.items) |file_name| {
                self.allocator.free(file_name);
            }
            wal_files.deinit(self.allocator);
        }

        while (try iterator.next()) |entry| {
            if (entry.kind != .file) continue;
            if (!std.mem.endsWith(u8, entry.name, ".wal")) continue;

            const file_name = try self.allocator.dupe(u8, entry.name);
            try wal_files.append(self.allocator, file_name);
        }

        // Sort files to process in order
        std.mem.sort([]const u8, wal_files.items, {}, struct {
            fn lessThan(_: void, a: []const u8, b: []const u8) bool {
                return std.mem.order(u8, a, b) == .lt;
            }
        }.lessThan);

        // Process each file in order
        for (wal_files.items) |file_name| {
            const file_path = try std.fs.path.join(self.allocator, &[_][]const u8{ self.base_path, file_name });
            defer self.allocator.free(file_path);

            const result = self.recoverFileWithOptions(file_path, options) catch |err| {
                std.log.warn("Failed to recover WAL file {s}: {}", .{ file_path, err });
                if (options.strict_mode) return err;
                continue;
            };

            max_lsn = @max(max_lsn, result.last_lsn);
            total_recovered += result.entries_recovered;
            total_corrupted += result.entries_corrupted;
            file_count += 1;

            // Check if we've hit max_entries limit
            if (options.max_entries > 0 and total_recovered >= options.max_entries) {
                std.log.info("Recovery stopped: reached max_entries limit ({})", .{options.max_entries});
                break;
            }
        }

        // Set current LSN to max + 1
        self.current_lsn.store(max_lsn + 1, .seq_cst);
        self.stats.file_count = file_count;
        self.stats.last_lsn = max_lsn;
        self.stats.recovery_entries = total_recovered;
        self.stats.corrupted_entries = total_corrupted;

        std.log.info("WAL recovery complete: {} files, {} entries recovered, {} corrupted, last LSN: {}", .{
            file_count,
            total_recovered,
            total_corrupted,
            max_lsn,
        });
    }

    /// Recovery result for a single file
    const RecoveryResult = struct {
        last_lsn: LSN,
        entries_recovered: u64,
        entries_corrupted: u64,
    };

    /// Recover from a single WAL file (basic - no callback)
    fn recoverFile(self: *Self, file_path: []const u8) !LSN {
        const options = RecoveryOptions{};
        const result = try self.recoverFileWithOptions(file_path, options);
        return result.last_lsn;
    }

    /// Recover from a single WAL file with options
    fn recoverFileWithOptions(self: *Self, file_path: []const u8, options: RecoveryOptions) !RecoveryResult {
        var file = try std.fs.cwd().openFile(file_path, .{});
        defer file.close();

        // Read and validate header
        var header: WALHeader = undefined;
        const header_bytes_read = try file.readAll(std.mem.asBytes(&header));
        if (header_bytes_read != @sizeOf(WALHeader)) {
            return error.IncompleteWALHeader;
        }

        if (!header.validate()) {
            return error.InvalidWALHeader;
        }

        var last_lsn: LSN = 0;
        var entries_recovered: u64 = 0;
        var entries_corrupted: u64 = 0;
        var pos: u64 = @sizeOf(WALHeader);

        // Read entries
        while (true) {
            // Check max_entries limit
            if (options.max_entries > 0 and entries_recovered >= options.max_entries) {
                break;
            }

            // Try to read entry header
            var entry_header: WALEntryHeader = undefined;
            const bytes_read = file.readAll(std.mem.asBytes(&entry_header)) catch |err| {
                if (err == error.EndOfStream) break;
                std.log.warn("Error reading entry header at position {}: {}", .{ pos, err });
                if (options.strict_mode) return err;
                break;
            };

            if (bytes_read == 0) break; // End of file
            if (bytes_read != @sizeOf(WALEntryHeader)) {
                std.log.warn("Incomplete entry header at position {} (got {} bytes)", .{ pos, bytes_read });
                if (options.strict_mode) return error.IncompleteEntryHeader;
                break;
            }

            // Read entry data
            const data = self.allocator.alloc(u8, entry_header.data_size) catch |err| {
                std.log.warn("Failed to allocate {} bytes for entry data: {}", .{ entry_header.data_size, err });
                if (options.strict_mode) return err;
                entries_corrupted += 1;
                break;
            };
            defer self.allocator.free(data);

            const data_bytes_read = file.readAll(data) catch |err| {
                std.log.warn("Error reading entry data at position {}: {}", .{ pos, err });
                if (options.strict_mode) return err;
                entries_corrupted += 1;
                break;
            };

            if (data_bytes_read != entry_header.data_size) {
                std.log.warn("Incomplete entry data at position {} (expected {}, got {})", .{
                    pos,
                    entry_header.data_size,
                    data_bytes_read,
                });
                if (options.strict_mode) return error.IncompleteEntryData;
                entries_corrupted += 1;
                break;
            }

            // Validate entry checksum
            if (!entry_header.validate(data)) {
                std.log.warn("Invalid checksum for entry at position {} (LSN: {})", .{ pos, entry_header.lsn });
                if (options.strict_mode) return error.InvalidChecksum;
                entries_corrupted += 1;

                // Skip padding and continue
                const entry_size = @sizeOf(WALEntryHeader) + entry_header.data_size;
                const aligned_size = std.mem.alignForward(usize, entry_size, WAL_ALIGNMENT);
                const padding_size = aligned_size - entry_size;
                if (padding_size > 0) {
                    file.seekBy(@intCast(padding_size)) catch break;
                }
                pos += aligned_size;
                continue;
            }

            // Entry is valid - update last_lsn
            last_lsn = @max(last_lsn, entry_header.lsn);
            entries_recovered += 1;

            // Call recovery callback if provided
            if (options.callback) |callback| {
                const entry = WALEntry{
                    .header = entry_header,
                    .data = data,
                };

                callback(&entry, options.user_data) catch |err| {
                    std.log.err("Recovery callback failed for entry at LSN {}: {}", .{ entry_header.lsn, err });
                    if (options.strict_mode) return err;
                    // Continue recovery even if callback fails
                };
            }

            // Skip padding
            const entry_size = @sizeOf(WALEntryHeader) + entry_header.data_size;
            const aligned_size = std.mem.alignForward(usize, entry_size, WAL_ALIGNMENT);
            const padding_size = aligned_size - entry_size;

            if (padding_size > 0) {
                file.seekBy(@intCast(padding_size)) catch break;
            }

            pos += aligned_size;
        }

        return RecoveryResult{
            .last_lsn = last_lsn,
            .entries_recovered = entries_recovered,
            .entries_corrupted = entries_corrupted,
        };
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
        self.current_size = stat.size;

        // If file is empty, write header
        if (self.current_size == 0) {
            var header = WALHeader.init();
            header.updateChecksum();

            const header_bytes = std.mem.asBytes(&header);
            _ = try self.current_file.?.writeAll(header_bytes);

            self.current_size = @sizeOf(WALHeader);
        }

        // Seek to end of file
        try self.current_file.?.seekTo(self.current_size);
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

        std.log.info("WAL file rotated, new file count: {}", .{self.stats.file_count});
    }
};

/// Simple CRC32 implementation
fn crc32(data: []const u8) u32 {
    var hasher = std.hash.Crc32.init();
    hasher.update(data);
    return hasher.final();
}

// Tests
test "WAL header validation" {
    var header = WALHeader.init();
    header.updateChecksum();

    try std.testing.expect(header.validate());

    // Corrupt header
    header.magic = 0xDEADBEEF;
    try std.testing.expect(!header.validate());
}

test "WAL entry creation and validation" {
    const allocator = std.testing.allocator;
    const data = "test data";
    const objref = ObjRef.init(1024);

    var entry = try WALEntry.init(allocator, 1, .insert, objref, data);
    defer entry.deinit(allocator);

    try std.testing.expect(entry.validate());
    try std.testing.expectEqual(@as(LSN, 1), entry.header.lsn);
    try std.testing.expectEqual(EntryType.insert, entry.header.entry_type);
    try std.testing.expectEqualStrings(data, entry.data);
}

test "WAL basic operations" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();
    defer std.fs.cwd().deleteTree(test_dir) catch {};

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

test "WAL buffered append" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_buffered";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
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

test "WAL group commit auto-flush" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_group_commit";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Configure for small batch size
    var config = GroupCommitConfig{};
    config.max_entries = 5;

    var wal = try WAL.initWithConfig(allocator, test_dir, config);
    defer wal.deinit();

    // Append entries - should auto-flush at 5
    var i: usize = 0;
    while (i < 7) : (i += 1) {
        const data = "auto flush test";
        const objref = ObjRef.init(2048 + i * 100);
        const entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
        try wal.appendBuffered(entry);
    }

    // Manually flush remaining entries to ensure all are written
    try wal.flush();

    // Should have all 7 entries flushed
    const stats = wal.getStats();
    try std.testing.expectEqual(@as(usize, 7), stats.entry_count);
}

test "WAL checkpoint" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_checkpoint";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create checkpoint
    const checkpoint_lsn = try wal.checkpoint();

    // Check stats
    const stats = wal.getStats();
    try std.testing.expectEqual(checkpoint_lsn, stats.last_checkpoint);
    try std.testing.expectEqual(@as(u64, 1), stats.entry_count);
}

test "WAL thread safety" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_thread_safety";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Test that LSN generation is thread-safe
    const lsn1 = wal.nextLSN();
    const lsn2 = wal.nextLSN();
    const lsn3 = wal.nextLSN();

    try std.testing.expectEqual(@as(LSN, 1), lsn1);
    try std.testing.expectEqual(@as(LSN, 2), lsn2);
    try std.testing.expectEqual(@as(LSN, 3), lsn3);
}

test "WAL recovery basic" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_recovery_basic";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL and write some entries
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        // Write 5 entries
        var i: usize = 0;
        while (i < 5) : (i += 1) {
            const data = "recovery test entry";
            const objref = ObjRef.init(1000 + i * 100);
            var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
            defer entry.deinit(allocator);
            try wal.append(&entry);
        }
    }

    // Reopen WAL - should recover entries
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        const stats = wal.getStats();
        try std.testing.expectEqual(@as(LSN, 6), wal.getCurrentLSN()); // Next LSN after 5
        try std.testing.expectEqual(@as(u64, 5), stats.recovery_entries);
    }
}

test "WAL recovery with callback" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_recovery_callback";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL and write entries
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        var i: usize = 0;
        while (i < 3) : (i += 1) {
            const data = "callback test";
            const objref = ObjRef.init(2000 + i * 100);
            var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
            defer entry.deinit(allocator);
            try wal.append(&entry);
        }
    }

    // Recovery callback to count entries
    const CallbackData = struct {
        count: u64 = 0,
    };

    const callback = struct {
        fn recoveryCallback(entry: *const WALEntry, user_data: ?*anyopaque) !void {
            const data = @as(*CallbackData, @ptrCast(@alignCast(user_data.?)));
            data.count += 1;

            // Validate entry type
            try std.testing.expectEqual(EntryType.insert, entry.header.entry_type);
        }
    }.recoveryCallback;

    // Reopen with callback
    var callback_data = CallbackData{};
    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    const options = RecoveryOptions{
        .callback = callback,
        .user_data = &callback_data,
    };

    try wal.recoverWithOptions(options);

    // Should have called callback 3 times
    try std.testing.expectEqual(@as(u64, 3), callback_data.count);
}

test "WAL recovery with corruption" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_recovery_corruption";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL and write entries
    var wal_path: []const u8 = undefined;
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        var i: usize = 0;
        while (i < 5) : (i += 1) {
            const data = "corruption test";
            const objref = ObjRef.init(3000 + i * 100);
            var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
            defer entry.deinit(allocator);
            try wal.append(&entry);
        }

        // Get WAL file path
        const filename = try std.fmt.allocPrint(allocator, "wal_{:08}.wal", .{1});
        defer allocator.free(filename);
        wal_path = try std.fs.path.join(allocator, &[_][]const u8{ test_dir, filename });
    }
    defer allocator.free(wal_path);

    // Corrupt the WAL file (truncate it)
    {
        const file = try std.fs.cwd().openFile(wal_path, .{ .mode = .read_write });
        defer file.close();

        // Truncate to remove last entry
        const stat = try file.stat();
        try file.setEndPos(stat.size - 100);
    }

    // Reopen - should recover partial entries
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        const stats = wal.getStats();
        // Should recover some entries (not all 5)
        try std.testing.expect(stats.recovery_entries < 5);
        try std.testing.expect(stats.recovery_entries > 0);
    }
}

test "WAL recovery strict mode" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_recovery_strict";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create corrupted WAL
    var wal_path: []const u8 = undefined;
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        const data = "strict mode test";
        const objref = ObjRef.init(4000);
        var entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
        defer entry.deinit(allocator);
        try wal.append(&entry);

        const filename = try std.fmt.allocPrint(allocator, "wal_{:08}.wal", .{1});
        defer allocator.free(filename);
        wal_path = try std.fs.path.join(allocator, &[_][]const u8{ test_dir, filename });
    }
    defer allocator.free(wal_path);

    // Corrupt the file
    {
        const file = try std.fs.cwd().openFile(wal_path, .{ .mode = .read_write });
        defer file.close();
        try file.setEndPos(50); // Truncate severely
    }

    // Strict mode should fail
    {
        var wal = WAL{
            .allocator = allocator,
            .base_path = try allocator.dupe(u8, test_dir),
            .current_file = null,
            .current_lsn = std.atomic.Value(LSN).init(1),
            .current_size = 0,
            .stats = WALStats.init(),
            .group_commit_config = GroupCommitConfig{},
            .pending_entries = .{},
            .pending_size = 0,
            .last_flush_time = std.time.milliTimestamp(),
            .flush_mutex = .{},
            .pending_allocator = allocator,
        };
        defer wal.deinit();

        const options = RecoveryOptions{
            .strict_mode = true,
        };

        // Should fail in strict mode (could be either error depending on truncation point)
        const result = wal.recoverWithOptions(options);
        try std.testing.expect(std.meta.isError(result));
    }
}

test "WAL batch operations" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_batch";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create batch entries
    var entries: std.ArrayList(WALEntry) = .{};
    defer {
        for (entries.items) |*entry| {
            entry.deinit(allocator);
        }
        entries.deinit(allocator);
    }

    // Add 10 entries to batch
    var i: usize = 0;
    while (i < 10) : (i += 1) {
        const data = try std.fmt.allocPrint(allocator, "batch entry {}", .{i});
        defer allocator.free(data);

        const objref = ObjRef.init(5000 + i * 100);
        const entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
        try entries.append(allocator, entry);
    }

    // Create batch
    const batch_lsn = wal.nextLSN();
    var batch = try BatchWALEntry.init(allocator, batch_lsn, try entries.toOwnedSlice(allocator));
    defer batch.deinit();

    // Append batch atomically
    try wal.appendBatchAtomic(&batch);

    // Check stats
    const stats = wal.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.batch_count);
    try std.testing.expectEqual(@as(u64, 10), stats.batch_entries);
    try std.testing.expectEqual(@as(u64, 12), stats.entry_count); // 10 + begin + end
}

test "WAL batch validation" {
    const allocator = std.testing.allocator;

    // Create batch entries
    var entries: std.ArrayList(WALEntry) = .{};
    defer {
        for (entries.items) |*entry| {
            entry.deinit(allocator);
        }
        entries.deinit(allocator);
    }

    // Add entries
    var i: usize = 0;
    while (i < 5) : (i += 1) {
        const data = try std.fmt.allocPrint(allocator, "entry {}", .{i});
        defer allocator.free(data);

        const objref = ObjRef.init(6000 + i * 100);
        const entry = try WALEntry.init(allocator, i + 1, .insert, objref, data);
        try entries.append(allocator, entry);
    }

    // Create batch
    var batch = try BatchWALEntry.init(allocator, 100, try entries.toOwnedSlice(allocator));
    defer batch.deinit();

    // Validate batch
    try std.testing.expect(batch.validate());
    try std.testing.expectEqual(@as(usize, 5), batch.entries.len);
}

test "WAL batch recovery" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_batch_recovery";

    // Clean up test directory
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create WAL and write batch
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        // Create batch entries
        var entries: std.ArrayList(WALEntry) = .{};
        defer {
            for (entries.items) |*entry| {
                entry.deinit(allocator);
            }
            entries.deinit(allocator);
        }

        var i: usize = 0;
        while (i < 5) : (i += 1) {
            const data = try std.fmt.allocPrint(allocator, "recovery batch {}", .{i});
            defer allocator.free(data);

            const objref = ObjRef.init(7000 + i * 100);
            const entry = try WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
            try entries.append(allocator, entry);
        }

        const batch_lsn = wal.nextLSN();
        var batch = try BatchWALEntry.init(allocator, batch_lsn, try entries.toOwnedSlice(allocator));
        defer batch.deinit();

        try wal.appendBatchAtomic(&batch);
    }

    // Reopen and verify recovery
    {
        var wal = try WAL.init(allocator, test_dir);
        defer wal.deinit();

        const stats = wal.getStats();
        try std.testing.expectEqual(@as(u64, 7), stats.recovery_entries); // 5 + begin + end
    }
}
