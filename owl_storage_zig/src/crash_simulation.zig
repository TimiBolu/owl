//! Zig-level Crash Simulation for Deterministic Testing
//!
//! Task 2.2.2: Add Zig-level crash simulation (Zig)
//!
//! This module provides controlled corruption injection and crash simulation
//! capabilities at the Zig storage layer. It enables deterministic testing
//! of crash recovery scenarios with various corruption patterns.
//!
//! Features:
//! - Deterministic failure injection based on simulation seed
//! - Controlled corruption injection for testing
//! - Partial write simulation for crash scenarios
//! - Recovery testing with various corruption patterns
//! - Integration with existing Arena, WAL, and Object components

const std = @import("std");
const Allocator = std.mem.Allocator;
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const Arena = @import("arena.zig").Arena;
const DeterministicArena = @import("deterministic_arena.zig").DeterministicArena;
const WAL = @import("wal.zig").WAL;
const WALEntry = @import("wal.zig").WALEntry;

/// Crash simulation modes
pub const CrashMode = enum {
    /// No crash simulation (normal operation)
    none,
    /// Crash before write operations complete
    before_write,
    /// Crash during write operations (partial writes)
    during_write,
    /// Crash after write but before sync
    after_write_before_sync,
    /// Crash after sync (data should survive)
    after_sync,
    /// Random crash timing based on seed
    random_timing,
};

/// Corruption types for testing
pub const CorruptionType = enum {
    /// No corruption
    none,
    /// Flip random bits in data
    bit_flip,
    /// Zero out random bytes
    zero_bytes,
    /// Truncate data at random position
    truncate,
    /// Corrupt object headers
    header_corruption,
    /// Corrupt WAL entries
    wal_corruption,
    /// Corrupt checksums
    checksum_corruption,
};

/// Failure injection configuration
pub const FailureConfig = struct {
    /// Enable crash simulation
    crash_enabled: bool = false,
    /// Crash mode to use
    crash_mode: CrashMode = .none,
    /// Probability of crash (0.0 to 1.0)
    crash_probability: f64 = 0.0,
    /// Enable corruption injection
    corruption_enabled: bool = false,
    /// Type of corruption to inject
    corruption_type: CorruptionType = .none,
    /// Probability of corruption (0.0 to 1.0)
    corruption_probability: f64 = 0.0,
    /// Maximum number of bytes to corrupt
    max_corruption_bytes: u32 = 16,
    /// Seed for deterministic behavior
    seed: u64 = 0,
};

/// Crash simulation statistics
pub const CrashStats = struct {
    /// Total number of operations attempted
    operations_attempted: u64 = 0,
    /// Number of crashes simulated
    crashes_simulated: u64 = 0,
    /// Number of corruptions injected
    corruptions_injected: u64 = 0,
    /// Number of partial writes simulated
    partial_writes: u64 = 0,
    /// Number of recovery attempts
    recovery_attempts: u64 = 0,
    /// Number of successful recoveries
    successful_recoveries: u64 = 0,
    /// Number of corruption detections
    corruptions_detected: u64 = 0,
};

/// Deterministic Random Number Generator for crash simulation
pub const CrashRNG = struct {
    state: u64,

    const MULTIPLIER: u64 = 1103515245;
    const INCREMENT: u64 = 12345;
    const MODULUS: u64 = 1 << 31;

    pub fn init(seed: u64) CrashRNG {
        return .{ .state = seed };
    }

    pub fn next(self: *CrashRNG) u32 {
        self.state = (MULTIPLIER *% self.state +% INCREMENT) % MODULUS;
        return @intCast(self.state);
    }

    pub fn nextFloat(self: *CrashRNG) f64 {
        return @as(f64, @floatFromInt(self.next())) / @as(f64, @floatFromInt(MODULUS));
    }

    pub fn nextRange(self: *CrashRNG, max: u32) u32 {
        if (max == 0) return 0;
        return self.next() % max;
    }

    pub fn nextBool(self: *CrashRNG) bool {
        return (self.next() & 1) == 1;
    }

    pub fn reset(self: *CrashRNG, seed: u64) void {
        self.state = seed;
    }
};

/// Crash simulator for Zig storage layer
pub const CrashSimulator = struct {
    /// Configuration for failure injection
    config: FailureConfig,
    /// Random number generator for deterministic behavior
    rng: CrashRNG,
    /// Statistics tracking
    stats: CrashStats,
    /// Whether system is currently crashed
    is_crashed: bool,
    /// Allocator for internal operations
    allocator: Allocator,
    /// Track corrupted offsets (just for statistics)
    corrupted_offsets: std.AutoHashMap(u64, bool),

    const Self = @This();

    /// Initialize crash simulator
    pub fn init(allocator: Allocator, config: FailureConfig) Self {
        return .{
            .config = config,
            .rng = CrashRNG.init(config.seed),
            .stats = CrashStats{},
            .is_crashed = false,
            .allocator = allocator,
            .corrupted_offsets = std.AutoHashMap(u64, bool).init(allocator),
        };
    }

    /// Clean up crash simulator
    pub fn deinit(self: *Self) void {
        self.corrupted_offsets.deinit();
    }

    /// Reset simulator state (for test repeatability)
    pub fn reset(self: *Self) void {
        self.rng.reset(self.config.seed);
        self.stats = CrashStats{};
        self.is_crashed = false;

        // Clear corrupted offsets
        self.corrupted_offsets.clearRetainingCapacity();
    }

    /// Check if system is currently crashed
    pub fn isCrashed(self: *const Self) bool {
        return self.is_crashed;
    }

    /// Simulate a crash
    pub fn crash(self: *Self) void {
        self.is_crashed = true;
        self.stats.crashes_simulated += 1;
    }

    /// Recover from crash
    pub fn recover(self: *Self) void {
        self.is_crashed = false;
        self.stats.recovery_attempts += 1;
    }

    /// Check if operation should crash based on configuration
    pub fn shouldCrash(self: *Self, operation: []const u8) bool {
        if (!self.config.crash_enabled or self.is_crashed) {
            return false;
        }

        self.stats.operations_attempted += 1;

        // Categorize operations
        const is_write_op = std.mem.eql(u8, operation, "arena_write") or
            std.mem.eql(u8, operation, "arena_write_post") or
            std.mem.eql(u8, operation, "wal_append") or
            std.mem.eql(u8, operation, "wal_append_post");

        const is_alloc_op = std.mem.eql(u8, operation, "arena_alloc");

        return switch (self.config.crash_mode) {
            .none => false,
            .before_write => (is_write_op or is_alloc_op) and self.rng.nextFloat() < self.config.crash_probability,
            .during_write => is_write_op and self.rng.nextFloat() < self.config.crash_probability,
            .after_write_before_sync => is_write_op and self.rng.nextFloat() < self.config.crash_probability,
            .after_sync => false, // Never crash after sync (data should survive)
            .random_timing => self.rng.nextFloat() < self.config.crash_probability,
        };
    }

    /// Check if data should be corrupted
    pub fn shouldCorrupt(self: *Self) bool {
        if (!self.config.corruption_enabled or self.is_crashed) {
            return false;
        }

        return self.rng.nextFloat() < self.config.corruption_probability;
    }

    /// Inject corruption into data
    pub fn injectCorruption(self: *Self, data: []u8, offset: u64) ![]u8 {
        if (!self.shouldCorrupt()) {
            return data;
        }

        self.stats.corruptions_injected += 1;

        // Create corrupted copy
        const corrupted = try self.allocator.dupe(u8, data);

        switch (self.config.corruption_type) {
            .none => {},
            .bit_flip => try self.corruptBitFlip(corrupted),
            .zero_bytes => try self.corruptZeroBytes(corrupted),
            .truncate => return try self.corruptTruncate(corrupted),
            .header_corruption => try self.corruptHeader(corrupted),
            .wal_corruption => try self.corruptWAL(corrupted),
            .checksum_corruption => try self.corruptChecksum(corrupted),
        }

        // Track corrupted offset for statistics
        try self.corrupted_offsets.put(offset, true);

        return corrupted;
    }

    /// Simulate partial write (crash during write)
    pub fn simulatePartialWrite(self: *Self, data: []const u8) ![]u8 {
        if (self.config.crash_mode != .during_write or !self.shouldCrash("arena_write")) {
            return try self.allocator.dupe(u8, data);
        }

        self.stats.partial_writes += 1;
        self.crash();

        // Return only partial data
        const partial_size = if (data.len > 1)
            self.rng.nextRange(@intCast(data.len - 1)) + 1
        else
            data.len;
        const partial_data = try self.allocator.alloc(u8, partial_size);
        @memcpy(partial_data, data[0..partial_size]);

        return partial_data;
    }

    /// Corrupt data by flipping random bits
    fn corruptBitFlip(self: *Self, data: []u8) !void {
        const num_flips = @min(self.config.max_corruption_bytes, data.len);

        var i: u32 = 0;
        while (i < num_flips) : (i += 1) {
            const byte_idx = self.rng.nextRange(@intCast(data.len));
            const bit_idx = self.rng.nextRange(8);
            data[byte_idx] ^= (@as(u8, 1) << @intCast(bit_idx));
        }
    }

    /// Corrupt data by zeroing random bytes
    fn corruptZeroBytes(self: *Self, data: []u8) !void {
        const num_zeros = @min(self.config.max_corruption_bytes, data.len);

        var i: u32 = 0;
        while (i < num_zeros) : (i += 1) {
            const byte_idx = self.rng.nextRange(@intCast(data.len));
            data[byte_idx] = 0;
        }
    }

    /// Corrupt data by truncating at random position
    fn corruptTruncate(self: *Self, data: []u8) ![]u8 {
        if (data.len == 0) return data;

        const truncate_pos = self.rng.nextRange(@intCast(data.len));
        const truncated = try self.allocator.alloc(u8, truncate_pos);
        @memcpy(truncated, data[0..truncate_pos]);

        self.allocator.free(data);
        return truncated;
    }

    /// Corrupt object header specifically
    fn corruptHeader(self: *Self, data: []u8) !void {
        if (data.len < ObjectHeader.headerSize()) return;

        // Corrupt header fields
        const header_bytes = data[0..ObjectHeader.headerSize()];

        // Corrupt type_tag
        if (self.rng.nextBool()) {
            header_bytes[0] = @truncate(self.rng.next());
            header_bytes[1] = @truncate(self.rng.next());
        }

        // Corrupt size field
        if (self.rng.nextBool()) {
            const size_offset = 20; // Offset of size field in ObjectHeader
            header_bytes[size_offset] = @truncate(self.rng.next());
            header_bytes[size_offset + 1] = @truncate(self.rng.next());
            header_bytes[size_offset + 2] = @truncate(self.rng.next());
            header_bytes[size_offset + 3] = @truncate(self.rng.next());
        }
    }

    /// Corrupt WAL entry data
    fn corruptWAL(self: *Self, data: []u8) !void {
        // Corrupt WAL entry header if data is large enough
        if (data.len >= 32) { // WAL entry header size
            // Corrupt LSN
            if (self.rng.nextBool()) {
                data[0] = @truncate(self.rng.next());
                data[1] = @truncate(self.rng.next());
                data[2] = @truncate(self.rng.next());
                data[3] = @truncate(self.rng.next());
            }

            // Corrupt entry type
            if (self.rng.nextBool()) {
                data[8] = @truncate(self.rng.nextRange(255));
            }
        }

        // Also apply general bit flips
        try self.corruptBitFlip(data);
    }

    /// Corrupt checksum fields
    fn corruptChecksum(self: *Self, data: []u8) !void {
        // Look for checksum patterns and corrupt them
        if (data.len >= 4) {
            // Corrupt potential checksum at end of data
            const checksum_pos = data.len - 4;
            data[checksum_pos] = @truncate(self.rng.next());
            data[checksum_pos + 1] = @truncate(self.rng.next());
            data[checksum_pos + 2] = @truncate(self.rng.next());
            data[checksum_pos + 3] = @truncate(self.rng.next());
        }

        // If this looks like an object header, corrupt the header checksum
        if (data.len >= ObjectHeader.headerSize()) {
            const checksum_offset = 28; // Offset of checksum in ObjectHeader
            data[checksum_offset] = @truncate(self.rng.next());
            data[checksum_offset + 1] = @truncate(self.rng.next());
            data[checksum_offset + 2] = @truncate(self.rng.next());
            data[checksum_offset + 3] = @truncate(self.rng.next());
        }
    }

    /// Get corruption statistics
    pub fn getStats(self: *const Self) CrashStats {
        return self.stats;
    }

    /// Update configuration
    pub fn updateConfig(self: *Self, config: FailureConfig) void {
        self.config = config;
        self.rng.reset(config.seed);
    }
};

/// Crash-aware Arena wrapper
pub const CrashAwareArena = struct {
    /// Underlying arena
    arena: *Arena,
    /// Crash simulator
    crash_sim: *CrashSimulator,
    /// Allocator for internal operations
    allocator: Allocator,

    const Self = @This();

    /// Initialize crash-aware arena
    pub fn init(allocator: Allocator, arena: *Arena, crash_sim: *CrashSimulator) Self {
        return .{
            .arena = arena,
            .crash_sim = crash_sim,
            .allocator = allocator,
        };
    }

    /// Allocate with crash simulation
    pub fn alloc(self: *Self, size: u32) !ObjRef {
        if (self.crash_sim.shouldCrash("arena_alloc")) {
            self.crash_sim.crash();
            return error.SystemCrashed;
        }

        return self.arena.alloc(size);
    }

    /// Write with crash and corruption simulation
    pub fn write(self: *Self, objref: ObjRef, data: []const u8) !void {
        if (self.crash_sim.isCrashed()) {
            return error.SystemCrashed;
        }

        // Check for crash before write
        if (self.crash_sim.config.crash_mode == .before_write and
            self.crash_sim.shouldCrash("arena_write"))
        {
            self.crash_sim.crash();
            return error.SystemCrashed;
        }

        // Simulate partial write if configured
        const write_data = if (self.crash_sim.config.crash_mode == .during_write)
            try self.crash_sim.simulatePartialWrite(data)
        else
            try self.allocator.dupe(u8, data);
        defer self.allocator.free(write_data);

        // Inject corruption if configured
        const final_data = try self.crash_sim.injectCorruption(write_data, objref.getOffset());
        defer if (final_data.ptr != write_data.ptr) self.allocator.free(final_data);

        // Perform the write
        try self.arena.write(objref, final_data);

        // Check for crash after write but before sync
        if (self.crash_sim.config.crash_mode == .after_write_before_sync and
            self.crash_sim.shouldCrash("arena_write_post"))
        {
            self.crash_sim.crash();
            return error.SystemCrashed;
        }
    }

    /// Read with corruption detection
    pub fn read(self: *Self, objref: ObjRef, allocator: Allocator) !Object {
        if (self.crash_sim.isCrashed()) {
            return error.SystemCrashed;
        }

        // Check if this offset was corrupted
        if (self.crash_sim.corrupted_offsets.contains(objref.getOffset())) {
            self.crash_sim.stats.corruptions_detected += 1;

            // Read the potentially corrupted data from arena
            const result = self.arena.read(objref, allocator);

            // Corruption might cause read failures
            return result catch |err| {
                // Expected failure due to corruption
                return err;
            };
        }

        return self.arena.read(objref, allocator);
    }

    /// Delete with crash simulation
    pub fn delete(self: *Self, objref: ObjRef) !void {
        if (self.crash_sim.isCrashed()) {
            return error.SystemCrashed;
        }

        if (self.crash_sim.shouldCrash("arena_delete")) {
            self.crash_sim.crash();
            return error.SystemCrashed;
        }

        return self.arena.delete(objref);
    }

    /// Get arena statistics
    pub fn stats(self: *const Self) Arena.ArenaStats {
        return self.arena.stats();
    }

    /// Get crash simulation statistics
    pub fn crashStats(self: *const Self) CrashStats {
        return self.crash_sim.getStats();
    }
};

/// Crash-aware WAL wrapper
pub const CrashAwareWAL = struct {
    /// Underlying WAL
    wal: *WAL,
    /// Crash simulator
    crash_sim: *CrashSimulator,
    /// Allocator for internal operations
    allocator: Allocator,

    const Self = @This();

    /// Initialize crash-aware WAL
    pub fn init(allocator: Allocator, wal: *WAL, crash_sim: *CrashSimulator) Self {
        return .{
            .wal = wal,
            .crash_sim = crash_sim,
            .allocator = allocator,
        };
    }

    /// Append entry with crash simulation
    pub fn append(self: *Self, entry: *const WALEntry) !void {
        if (self.crash_sim.isCrashed()) {
            return error.SystemCrashed;
        }

        // Check for crash before write
        if (self.crash_sim.config.crash_mode == .before_write and
            self.crash_sim.shouldCrash("wal_append"))
        {
            self.crash_sim.crash();
            return error.SystemCrashed;
        }

        // Simulate corruption in WAL entry
        var corrupted_entry = entry.*;
        var corrupted_data_allocated: ?[]u8 = null;
        if (self.crash_sim.shouldCorrupt()) {
            // Corrupt the entry data
            const corrupted_data = try self.crash_sim.injectCorruption(corrupted_entry.data, corrupted_entry.header.lsn);
            if (corrupted_data.ptr != corrupted_entry.data.ptr) {
                corrupted_data_allocated = corrupted_data;
            }
            corrupted_entry.data = corrupted_data;

            // Update checksum with corrupted data (this will make it invalid)
            corrupted_entry.header.updateChecksum(corrupted_data);
        }
        defer if (corrupted_data_allocated) |data| self.allocator.free(data);

        // Perform the append
        const result = self.wal.append(&corrupted_entry);

        // Check for crash after write but before sync
        if (self.crash_sim.config.crash_mode == .after_write_before_sync and
            self.crash_sim.shouldCrash("wal_append_post"))
        {
            self.crash_sim.crash();
            return error.SystemCrashed;
        }

        return result;
    }

    /// Checkpoint with crash simulation
    pub fn checkpoint(self: *Self) !WAL.LSN {
        if (self.crash_sim.isCrashed()) {
            return error.SystemCrashed;
        }

        if (self.crash_sim.shouldCrash("wal_checkpoint")) {
            self.crash_sim.crash();
            return error.SystemCrashed;
        }

        return self.wal.checkpoint();
    }

    /// Get WAL statistics
    pub fn getStats(self: *const Self) WAL.WALStats {
        return self.wal.getStats();
    }

    /// Get crash simulation statistics
    pub fn crashStats(self: *const Self) CrashStats {
        return self.crash_sim.getStats();
    }
};

// ============================================================================
// Tests
// ============================================================================

test "CrashRNG: deterministic behavior" {
    var rng1 = CrashRNG.init(12345);
    var rng2 = CrashRNG.init(12345);

    // Same seed should produce same sequence
    for (0..100) |_| {
        try std.testing.expectEqual(rng1.next(), rng2.next());
    }

    // Different seeds should produce different sequences
    var rng3 = CrashRNG.init(54321);
    rng1.reset(12345);

    var different_count: u32 = 0;
    for (0..100) |_| {
        if (rng1.next() != rng3.next()) {
            different_count += 1;
        }
    }

    try std.testing.expect(different_count > 80);
}

test "CrashSimulator: basic functionality" {
    const allocator = std.testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .random_timing,
        .crash_probability = 0.5,
        .seed = 42,
    };

    var sim = CrashSimulator.init(allocator, config);
    defer sim.deinit();

    try std.testing.expect(!sim.isCrashed());

    // Test crash simulation
    var crash_count: u32 = 0;
    for (0..100) |_| {
        if (sim.shouldCrash("test_op")) {
            sim.crash();
            crash_count += 1;
            sim.recover();
        }
    }

    // Should have some crashes (probabilistic, but with fixed seed should be deterministic)
    try std.testing.expect(crash_count > 0);
    try std.testing.expect(crash_count < 100);
}

test "CrashSimulator: corruption injection" {
    const allocator = std.testing.allocator;

    const config = FailureConfig{
        .corruption_enabled = true,
        .corruption_type = .bit_flip,
        .corruption_probability = 1.0, // Always corrupt
        .max_corruption_bytes = 4,
        .seed = 123,
    };

    var sim = CrashSimulator.init(allocator, config);
    defer sim.deinit();

    const original_data = "Hello, World!";
    const corrupted = try sim.injectCorruption(@constCast(original_data), 0);
    defer if (corrupted.ptr != original_data.ptr) allocator.free(corrupted);

    // Data should be different due to corruption
    try std.testing.expect(!std.mem.eql(u8, original_data, corrupted));
    try std.testing.expectEqual(@as(u64, 1), sim.stats.corruptions_injected);
}

test "CrashSimulator: partial write simulation" {
    const allocator = std.testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .during_write,
        .crash_probability = 1.0, // Always crash
        .seed = 456,
    };

    var sim = CrashSimulator.init(allocator, config);
    defer sim.deinit();

    const original_data = "This is a long piece of data for partial write testing";
    const partial = try sim.simulatePartialWrite(original_data);
    defer allocator.free(partial);

    // Should be crashed and have partial data
    try std.testing.expect(sim.isCrashed());
    try std.testing.expect(partial.len < original_data.len);
    try std.testing.expectEqual(@as(u64, 1), sim.stats.partial_writes);
}

test "CrashSimulator: reset functionality" {
    const allocator = std.testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_probability = 1.0,
        .seed = 789,
    };

    var sim = CrashSimulator.init(allocator, config);
    defer sim.deinit();

    // Cause some state changes
    _ = sim.shouldCrash("test");
    sim.crash();

    const stats_before = sim.getStats();
    try std.testing.expect(stats_before.operations_attempted > 0);
    try std.testing.expect(sim.isCrashed());

    // Reset should restore initial state
    sim.reset();

    const stats_after = sim.getStats();
    try std.testing.expectEqual(@as(u64, 0), stats_after.operations_attempted);
    try std.testing.expect(!sim.isCrashed());
}

fn createTestArena(allocator: std.mem.Allocator) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_crash_arena.dat" });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 1024 * 1024, 0); // 1MB, shard_id = 0
    return .{ .arena = arena, .dir = tmp };
}

test "CrashAwareArena: crash during allocation" {
    const allocator = std.testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .before_write,
        .crash_probability = 1.0, // Always crash
        .seed = 111,
    };

    var sim = CrashSimulator.init(allocator, config);
    defer sim.deinit();

    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &sim);

    // Should crash during allocation
    const result = crash_arena.alloc(100);
    try std.testing.expectError(error.SystemCrashed, result);
    try std.testing.expect(sim.isCrashed());
}

test "CrashAwareArena: corruption detection" {
    const allocator = std.testing.allocator;

    const config = FailureConfig{
        .corruption_enabled = true,
        .corruption_type = .header_corruption,
        .corruption_probability = 1.0, // Always corrupt
        .seed = 222,
    };

    var sim = CrashSimulator.init(allocator, config);
    defer sim.deinit();

    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &sim);

    // Create and write an object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try test_arena.arena.alloc(@intCast(data.len));

    // Write with corruption
    try crash_arena.write(objref, data);

    // Reading should detect corruption
    const read_result = crash_arena.read(objref, allocator);

    // Should either fail or detect corruption
    if (read_result) |read_obj| {
        var read_obj_mut = read_obj;
        read_obj_mut.deinit();
    } else |err| {
        // Expected - corruption should cause read failure
        try std.testing.expect(err == error.InsufficientData or
            err == error.ChecksumMismatch or
            err == error.SystemCrashed);
    }

    try std.testing.expectEqual(@as(u64, 1), sim.stats.corruptions_injected);
}

test "CrashAwareArena: recovery after crash" {
    const allocator = std.testing.allocator;

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .after_write_before_sync,
        .crash_probability = 1.0,
        .seed = 333,
    };

    var sim = CrashSimulator.init(allocator, config);
    defer sim.deinit();

    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &test_arena.arena, &sim);

    // Create a proper object with valid header
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, "test data", &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try test_arena.arena.alloc(@intCast(data.len));

    // Should crash after write
    const result = crash_arena.write(objref, data);
    try std.testing.expectError(error.SystemCrashed, result);
    try std.testing.expect(sim.isCrashed());

    // Recover and try again
    sim.recover();
    try std.testing.expect(!sim.isCrashed());

    // Disable crash for successful write
    sim.config.crash_enabled = false;
    try crash_arena.write(objref, data);

    // Should be able to read after recovery
    const read_data = try test_arena.arena.readAt(objref.getOffset(), data.len);
    try std.testing.expectEqualStrings(data, read_data);
}

test "CrashSimulator: different corruption types" {
    const allocator = std.testing.allocator;

    const corruption_types = [_]CorruptionType{
        .bit_flip,
        .zero_bytes,
        .truncate,
        .header_corruption,
        .checksum_corruption,
    };

    for (corruption_types) |corruption_type| {
        const config = FailureConfig{
            .corruption_enabled = true,
            .corruption_type = corruption_type,
            .corruption_probability = 1.0,
            .seed = 444,
        };

        var sim = CrashSimulator.init(allocator, config);
        defer sim.deinit();

        const original_data = "Test data for corruption type testing with sufficient length";
        const corrupted = try sim.injectCorruption(@constCast(original_data), 0);
        defer if (corrupted.ptr != original_data.ptr) allocator.free(corrupted);

        // Verify corruption occurred (except for truncate which changes length)
        if (corruption_type == .truncate) {
            try std.testing.expect(corrupted.len <= original_data.len);
        } else {
            try std.testing.expect(!std.mem.eql(u8, original_data, corrupted));
        }

        try std.testing.expectEqual(@as(u64, 1), sim.stats.corruptions_injected);
    }
}
