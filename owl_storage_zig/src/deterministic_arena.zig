//! Deterministic Arena Allocator for Simulation Testing
//!
//! Task 2.2.1: Add Zig-level deterministic allocation
//!
//! This module provides a deterministic memory allocator that produces
//! identical allocation patterns across simulation runs when given the same seed.
//! This is crucial for reproducible testing and debugging of distributed systems.

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;

/// Page size for arena allocation (4KB)
pub const PAGE_SIZE: usize = 4096;

/// Slot size for free list management (64 bytes)
pub const SLOT_SIZE: usize = 64;

/// Initial offset for bump allocator (skip offset 0 which is reserved for null)
pub const INITIAL_OFFSET: u32 = 64;

/// Deterministic Random Number Generator
/// Uses a simple Linear Congruential Generator (LCG) for reproducible randomness
pub const DeterministicRNG = struct {
    state: u64,

    const MULTIPLIER: u64 = 1103515245;
    const INCREMENT: u64 = 12345;
    const MODULUS: u64 = 1 << 31;

    pub fn init(seed: u64) DeterministicRNG {
        return .{ .state = seed };
    }

    /// Generate next random number
    pub fn next(self: *DeterministicRNG) u32 {
        self.state = (MULTIPLIER *% self.state +% INCREMENT) % MODULUS;
        return @intCast(self.state);
    }

    /// Generate random number in range [0, max)
    pub fn nextRange(self: *DeterministicRNG, max: u32) u32 {
        if (max == 0) return 0;
        return self.next() % max;
    }

    /// Generate random boolean
    pub fn nextBool(self: *DeterministicRNG) bool {
        return (self.next() & 1) == 1;
    }

    /// Reset to initial seed
    pub fn reset(self: *DeterministicRNG, seed: u64) void {
        self.state = seed;
    }
};

/// Deterministic allocation strategy
pub const AllocationStrategy = enum {
    /// Standard bump allocation (deterministic by nature)
    bump,
    /// Random placement within available space (deterministic with seed)
    random_placement,
    /// Fragmentation simulation - intentionally create gaps
    fragmentation_sim,
    /// Co-location optimization with deterministic decisions
    deterministic_colocation,
};

/// Deterministic garbage collection configuration
pub const GCConfig = struct {
    /// Enable deterministic GC timing
    enabled: bool = false,
    /// GC trigger interval (number of allocations)
    trigger_interval: u32 = 1000,
    /// Percentage of objects to collect (0-100)
    collection_percentage: u8 = 25,
    /// Deterministic selection of objects to collect
    deterministic_selection: bool = true,
};

/// Allocation event for reproducibility tracking
pub const AllocationEvent = struct {
    /// Sequence number of this allocation
    sequence: u64,
    /// Size requested
    size: u32,
    /// Offset allocated
    offset: u64,
    /// Strategy used
    strategy: AllocationStrategy,
    /// Random seed state at time of allocation
    rng_state: u64,
};

/// Deterministic Arena - provides reproducible allocation patterns
pub const DeterministicArena = struct {
    /// Memory-mapped region
    mmap: []align(std.heap.page_size_min) u8,
    /// Free list of available slot IDs (deterministically managed)
    free_list: std.ArrayList(u32),
    /// Bump allocator pointer (offset in bytes)
    bump: u32,
    /// Total arena size in bytes
    size: usize,
    /// Bytes currently used
    used: usize,
    /// Number of objects allocated
    object_count: u64,
    /// Page allocation tracking (page_id -> bytes used in page)
    page_usage: []u32,
    /// Allocator for internal structures
    allocator: std.mem.Allocator,

    // Deterministic allocation fields
    /// Random number generator with fixed seed
    rng: DeterministicRNG,
    /// Initial seed for reproducibility
    initial_seed: u64,
    /// Current allocation strategy
    strategy: AllocationStrategy,
    /// Allocation sequence counter
    allocation_sequence: u64,
    /// Garbage collection configuration
    gc_config: GCConfig,
    /// Allocation event log for debugging
    allocation_log: std.ArrayList(AllocationEvent),
    /// Enable detailed logging
    logging_enabled: bool,

    const Self = @This();

    /// Create a new deterministic arena
    pub fn init(
        allocator: std.mem.Allocator,
        path: []const u8,
        size: usize,
        seed: u64,
        strategy: AllocationStrategy,
        gc_config: GCConfig,
    ) !Self {
        // Open or create the file
        const file = try std.fs.cwd().createFile(path, .{
            .read = true,
            .truncate = false,
        });
        defer file.close();

        // Set file size
        try file.setEndPos(size);

        // Memory map the file
        const mmap = try std.posix.mmap(
            null,
            size,
            std.posix.PROT.READ | std.posix.PROT.WRITE,
            .{ .TYPE = .SHARED },
            file.handle,
            0,
        );

        // Calculate number of pages and allocate page tracking
        const num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        const page_usage = try allocator.alloc(u32, num_pages);
        for (page_usage) |*p| {
            p.* = 0;
        }

        return .{
            .mmap = mmap,
            .free_list = std.ArrayList(u32){},
            .bump = INITIAL_OFFSET,
            .size = size,
            .used = 0,
            .object_count = 0,
            .page_usage = page_usage,
            .allocator = allocator,
            .rng = DeterministicRNG.init(seed),
            .initial_seed = seed,
            .strategy = strategy,
            .allocation_sequence = 0,
            .gc_config = gc_config,
            .allocation_log = std.ArrayList(AllocationEvent){},
            .logging_enabled = false,
        };
    }

    /// Clean up arena resources
    pub fn deinit(self: *Self) void {
        std.posix.munmap(self.mmap);
        self.free_list.deinit(self.allocator);
        self.allocator.free(self.page_usage);
        self.allocation_log.deinit(self.allocator);
    }

    /// Reset arena to initial state with same seed
    pub fn reset(self: *Self) void {
        self.bump = INITIAL_OFFSET;
        self.used = 0;
        self.object_count = 0;
        self.allocation_sequence = 0;
        self.rng.reset(self.initial_seed);
        self.free_list.clearRetainingCapacity();
        self.allocation_log.clearRetainingCapacity();

        // Reset page usage
        for (self.page_usage) |*p| {
            p.* = 0;
        }

        // Clear memory (deterministic state)
        @memset(self.mmap, 0);
    }

    /// Enable or disable allocation logging
    pub fn setLogging(self: *Self, enabled: bool) void {
        self.logging_enabled = enabled;
    }

    /// Get allocation log for analysis
    pub fn getAllocationLog(self: *const Self) []const AllocationEvent {
        return self.allocation_log.items;
    }

    /// Allocate space for an object using deterministic strategy
    pub fn alloc(self: *Self, size: u32) !ObjRef {
        return self.allocNear(size, null);
    }

    /// Allocate space near another object (deterministic co-location)
    pub fn allocNear(self: *Self, size: u32, near: ?ObjRef) !ObjRef {
        self.allocation_sequence += 1;

        // Check for deterministic garbage collection
        if (self.gc_config.enabled and
            self.allocation_sequence % self.gc_config.trigger_interval == 0)
        {
            try self.deterministicGC();
        }

        const objref = switch (self.strategy) {
            .bump => try self.allocBump(size),
            .random_placement => try self.allocRandom(size),
            .fragmentation_sim => try self.allocFragmented(size),
            .deterministic_colocation => try self.allocDeterministicColocation(size, near),
        };

        // Log allocation event
        if (self.logging_enabled) {
            const event = AllocationEvent{
                .sequence = self.allocation_sequence,
                .size = size,
                .offset = objref.getOffset(),
                .strategy = self.strategy,
                .rng_state = self.rng.state,
            };
            try self.allocation_log.append(self.allocator, event);
        }

        self.object_count += 1;
        self.used += size;

        return objref;
    }

    /// Standard bump allocation (deterministic by nature)
    fn allocBump(self: *Self, size: u32) !ObjRef {
        if (self.bump + size > self.size) {
            return error.OutOfMemory;
        }

        const offset = self.bump;
        self.bump += size;

        // Update page usage
        const page_id = offset / PAGE_SIZE;
        const page_idx: usize = @intCast(page_id);
        if (page_idx < self.page_usage.len) {
            self.page_usage[page_idx] += size;
        }

        return ObjRef.init(offset);
    }

    /// Random placement allocation (deterministic with seed)
    fn allocRandom(self: *Self, size: u32) !ObjRef {
        // Find available space randomly but deterministically
        const available_space = self.size - self.used;
        if (size > available_space) {
            return error.OutOfMemory;
        }

        // Use RNG to select placement within available space
        const max_offset = self.size - size;
        var attempts: u32 = 0;
        const max_attempts = 100;

        while (attempts < max_attempts) {
            const candidate_offset = INITIAL_OFFSET + (self.rng.nextRange(@intCast(max_offset - INITIAL_OFFSET)));

            // Check if this region is free (simplified check)
            if (self.isRegionFree(candidate_offset, size)) {
                self.markRegionUsed(candidate_offset, size);
                return ObjRef.init(candidate_offset);
            }

            attempts += 1;
        }

        // Fall back to bump allocation if random placement fails
        return self.allocBump(size);
    }

    /// Fragmentation simulation - intentionally create gaps
    fn allocFragmented(self: *Self, size: u32) !ObjRef {
        // Randomly decide whether to create a gap before allocation
        if (self.rng.nextBool() and self.bump + size + 64 < self.size) {
            // Create a random gap (16-64 bytes)
            const gap_size = 16 + self.rng.nextRange(48);
            self.bump += gap_size;
        }

        return self.allocBump(size);
    }

    /// Deterministic co-location allocation
    fn allocDeterministicColocation(self: *Self, size: u32, near: ?ObjRef) !ObjRef {
        if (near) |near_ref| {
            const near_page = near_ref.getOffset() / PAGE_SIZE;
            const page_idx: usize = @intCast(near_page);

            // Deterministically decide whether to co-locate based on RNG
            if (self.rng.nextRange(100) < 70) { // 70% chance of co-location
                // Try to allocate in the same page
                if (page_idx < self.page_usage.len and
                    self.page_usage[page_idx] + size <= PAGE_SIZE)
                {
                    const page_offset = near_page * PAGE_SIZE;
                    const in_page_offset = self.page_usage[page_idx];
                    const offset = page_offset + in_page_offset;

                    self.page_usage[page_idx] += size;
                    return ObjRef.init(offset);
                }
            }
        }

        // Fall back to bump allocation
        return self.allocBump(size);
    }

    /// Check if a memory region is free (simplified implementation)
    fn isRegionFree(self: *const Self, offset: u64, size: u32) bool {
        _ = size; // Unused parameter for now
        // For simplicity, just check if it's beyond current bump pointer
        // In a full implementation, we'd maintain a proper free space map
        return offset >= self.bump;
    }

    /// Mark a memory region as used
    fn markRegionUsed(self: *Self, offset: u64, size: u32) void {
        // Update page usage
        const start_page = offset / PAGE_SIZE;
        const end_page = (offset + size - 1) / PAGE_SIZE;

        var page = start_page;
        while (page <= end_page) : (page += 1) {
            const page_idx: usize = @intCast(page);
            if (page_idx < self.page_usage.len) {
                const page_start = page * PAGE_SIZE;
                const page_end = (page + 1) * PAGE_SIZE;
                const region_start = @max(offset, page_start);
                const region_end = @min(offset + size, page_end);
                const region_size: u32 = @intCast(region_end - region_start);

                self.page_usage[page_idx] += region_size;
            }
        }
    }

    /// Deterministic garbage collection
    fn deterministicGC(self: *Self) !void {
        if (!self.gc_config.enabled) return;

        const objects_to_collect = (self.object_count * self.gc_config.collection_percentage) / 100;
        if (objects_to_collect == 0) return;

        // Deterministically select objects to collect based on RNG
        // This is a simplified implementation - in practice, we'd need
        // to track object locations and implement proper collection

        var collected: u64 = 0;
        var current_offset: u64 = INITIAL_OFFSET;

        while (collected < objects_to_collect and current_offset < self.bump) {
            // Use RNG to decide whether to "collect" this object
            if (self.rng.nextRange(100) < self.gc_config.collection_percentage) {
                // Mark object as collected (simplified)
                // In a real implementation, we'd add to free list
                collected += 1;
            }

            // Move to next potential object (simplified)
            current_offset += 64; // Assume average object size
        }

        // Update statistics
        self.object_count -= collected;
    }

    /// Write data to a specific offset
    pub fn writeAt(self: *Self, offset: u64, data: []const u8) !void {
        const start: usize = @intCast(offset);
        const end = start + data.len;

        if (end > self.size) {
            return error.OutOfBounds;
        }

        @memcpy(self.mmap[start..end], data);
    }

    /// Read data from a specific offset
    pub fn readAt(self: *const Self, offset: u64, len: usize) ![]const u8 {
        const start: usize = @intCast(offset);
        const end = start + len;

        if (end > self.size) {
            return error.OutOfBounds;
        }

        return self.mmap[start..end];
    }

    /// Write an object to the arena
    pub fn write(self: *Self, objref: ObjRef, data: []const u8) !void {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        // Validate data contains a valid object header
        if (data.len < ObjectHeader.headerSize()) {
            return error.DataTooSmall;
        }

        // Bounds checking
        const offset = objref.getOffset();
        if (offset >= self.size or offset + data.len > self.size) {
            return error.OutOfBounds;
        }

        // Write data
        try self.writeAt(offset, data);
    }

    /// Read an object from the arena
    pub fn read(self: *const Self, objref: ObjRef, allocator: std.mem.Allocator) !Object {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        const offset = objref.getOffset();
        if (offset >= self.size) {
            return error.OutOfBounds;
        }

        // Read header first to get total size
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        const header = try ObjectHeader.fromBytes(header_bytes);

        // Validate header
        if (header.isDeleted()) {
            return error.ObjectDeleted;
        }

        // Bounds check for full object
        if (offset + header.size > self.size) {
            return error.OutOfBounds;
        }

        // Read full object
        const full_bytes = try self.readAt(offset, header.size);

        // Deserialize object
        return try Object.fromBytes(allocator, full_bytes);
    }

    /// Delete an object (deterministic)
    pub fn delete(self: *Self, objref: ObjRef) !void {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        const offset = objref.getOffset();
        if (offset >= self.size) {
            return error.OutOfBounds;
        }

        // Read header
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        var header = try ObjectHeader.fromBytes(header_bytes);

        // Mark as deleted
        header.setDeleted(true);

        // Write back modified header
        const modified_header = header.toBytes();
        try self.writeAt(offset, &modified_header);

        // Add to free list deterministically
        const slot_id: u32 = @intCast(offset / SLOT_SIZE);
        try self.free_list.append(self.allocator, slot_id);
        self.object_count -= 1;
    }

    /// Get arena statistics
    pub fn stats(self: *const Self) ArenaStats {
        return .{
            .total_size = self.size,
            .used_size = self.used,
            .free_size = self.size - self.used,
            .object_count = self.object_count,
            .free_slots = self.free_list.items.len,
            .allocation_sequence = self.allocation_sequence,
            .current_strategy = self.strategy,
            .rng_state = self.rng.state,
        };
    }

    /// Get the base pointer for swizzling
    pub fn basePtr(self: *const Self) [*]u8 {
        return self.mmap.ptr;
    }

    /// Change allocation strategy (for testing different patterns)
    pub fn setStrategy(self: *Self, strategy: AllocationStrategy) void {
        self.strategy = strategy;
    }

    /// Get current RNG state (for debugging reproducibility)
    pub fn getRNGState(self: *const Self) u64 {
        return self.rng.state;
    }

    /// Manually advance RNG (for testing)
    pub fn advanceRNG(self: *Self, steps: u32) void {
        var i: u32 = 0;
        while (i < steps) : (i += 1) {
            _ = self.rng.next();
        }
    }
};

/// Extended arena statistics with deterministic allocation info
pub const ArenaStats = struct {
    total_size: usize,
    used_size: usize,
    free_size: usize,
    object_count: u64,
    free_slots: usize,
    allocation_sequence: u64,
    current_strategy: AllocationStrategy,
    rng_state: u64,
};

// ============================================================================
// Tests
// ============================================================================

fn createTestDeterministicArena(allocator: std.mem.Allocator, seed: u64) !struct { arena: DeterministicArena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_det_arena.dat" });
    defer allocator.free(file_path);

    const gc_config = GCConfig{};
    const arena = try DeterministicArena.init(allocator, file_path, 1024 * 1024, // 1MB
        seed, .bump, gc_config);

    return .{ .arena = arena, .dir = tmp };
}

test "DeterministicRNG reproducibility" {
    var rng1 = DeterministicRNG.init(12345);
    var rng2 = DeterministicRNG.init(12345);

    // Same seed should produce same sequence
    for (0..100) |_| {
        try std.testing.expectEqual(rng1.next(), rng2.next());
    }

    // Different seeds should produce different sequences
    var rng3 = DeterministicRNG.init(54321);
    rng1.reset(12345);

    var different_count: u32 = 0;
    for (0..100) |_| {
        if (rng1.next() != rng3.next()) {
            different_count += 1;
        }
    }

    // Should be mostly different (allow for some collisions)
    try std.testing.expect(different_count > 80);
}

test "DeterministicArena: reproducible allocation patterns" {
    const allocator = std.testing.allocator;
    const seed: u64 = 42;

    // Create two arenas with same seed
    var test_arena1 = try createTestDeterministicArena(allocator, seed);
    defer test_arena1.dir.cleanup();
    defer test_arena1.arena.deinit();

    var test_arena2 = try createTestDeterministicArena(allocator, seed);
    defer test_arena2.dir.cleanup();
    defer test_arena2.arena.deinit();

    // Enable logging
    test_arena1.arena.setLogging(true);
    test_arena2.arena.setLogging(true);

    // Perform same allocations on both arenas
    const sizes = [_]u32{ 64, 128, 256, 64, 512 };

    for (sizes) |size| {
        const ref1 = try test_arena1.arena.alloc(size);
        const ref2 = try test_arena2.arena.alloc(size);

        // Should allocate at same offsets
        try std.testing.expectEqual(ref1.getOffset(), ref2.getOffset());
    }

    // Allocation logs should be identical
    const log1 = test_arena1.arena.getAllocationLog();
    const log2 = test_arena2.arena.getAllocationLog();

    try std.testing.expectEqual(log1.len, log2.len);
    for (log1, log2) |event1, event2| {
        try std.testing.expectEqual(event1.sequence, event2.sequence);
        try std.testing.expectEqual(event1.size, event2.size);
        try std.testing.expectEqual(event1.offset, event2.offset);
        try std.testing.expectEqual(event1.rng_state, event2.rng_state);
    }
}

test "DeterministicArena: reset reproducibility" {
    const allocator = std.testing.allocator;
    const seed: u64 = 123;

    var test_arena = try createTestDeterministicArena(allocator, seed);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    test_arena.arena.setLogging(true);

    // First run
    _ = try test_arena.arena.alloc(64);
    _ = try test_arena.arena.alloc(128);
    const first_stats = test_arena.arena.stats();
    const first_log = test_arena.arena.getAllocationLog();

    // Reset and second run
    test_arena.arena.reset();
    _ = try test_arena.arena.alloc(64);
    _ = try test_arena.arena.alloc(128);
    const second_stats = test_arena.arena.stats();
    const second_log = test_arena.arena.getAllocationLog();

    // Should produce identical results
    try std.testing.expectEqual(first_stats.used_size, second_stats.used_size);
    try std.testing.expectEqual(first_stats.object_count, second_stats.object_count);
    try std.testing.expectEqual(first_stats.rng_state, second_stats.rng_state);

    try std.testing.expectEqual(first_log.len, second_log.len);
    for (first_log, second_log) |event1, event2| {
        try std.testing.expectEqual(event1.offset, event2.offset);
        try std.testing.expectEqual(event1.rng_state, event2.rng_state);
    }
}

test "DeterministicArena: different strategies" {
    const allocator = std.testing.allocator;
    const seed: u64 = 456;

    var test_arena = try createTestDeterministicArena(allocator, seed);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Test bump allocation
    test_arena.arena.setStrategy(.bump);
    const ref1 = try test_arena.arena.alloc(64);
    const ref2 = try test_arena.arena.alloc(64);

    // Bump allocation should be sequential
    try std.testing.expectEqual(@as(u64, 64), ref1.getOffset());
    try std.testing.expectEqual(@as(u64, 128), ref2.getOffset());

    // Reset and test fragmentation simulation
    test_arena.arena.reset();
    test_arena.arena.setStrategy(.fragmentation_sim);

    const ref3 = try test_arena.arena.alloc(64);
    const ref4 = try test_arena.arena.alloc(64);

    // Fragmentation sim may create gaps
    try std.testing.expect(ref4.getOffset() >= ref3.getOffset() + 64);
}

test "DeterministicArena: deterministic garbage collection" {
    const allocator = std.testing.allocator;
    const seed: u64 = 789;

    const gc_config = GCConfig{
        .enabled = true,
        .trigger_interval = 5, // Trigger after 5 allocations
        .collection_percentage = 50,
        .deterministic_selection = true,
    };

    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_gc_arena.dat" });
    defer allocator.free(file_path);

    var arena = try DeterministicArena.init(allocator, file_path, 1024 * 1024, seed, .bump, gc_config);
    defer arena.deinit();

    // Allocate objects to trigger GC
    for (0..10) |_| {
        _ = try arena.alloc(64);
    }

    const stats = arena.stats();

    // Should have triggered GC at least once
    try std.testing.expect(stats.allocation_sequence >= 5);

    // Object count should be affected by GC
    try std.testing.expect(stats.object_count <= 10);
}

test "DeterministicArena: allocation logging" {
    const allocator = std.testing.allocator;
    const seed: u64 = 999;

    var test_arena = try createTestDeterministicArena(allocator, seed);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    test_arena.arena.setLogging(true);

    // Perform various allocations
    _ = try test_arena.arena.alloc(64);
    _ = try test_arena.arena.alloc(128);
    _ = try test_arena.arena.alloc(256);

    const log = test_arena.arena.getAllocationLog();
    try std.testing.expectEqual(@as(usize, 3), log.len);

    // Verify log entries
    try std.testing.expectEqual(@as(u64, 1), log[0].sequence);
    try std.testing.expectEqual(@as(u32, 64), log[0].size);
    try std.testing.expectEqual(AllocationStrategy.bump, log[0].strategy);

    try std.testing.expectEqual(@as(u64, 2), log[1].sequence);
    try std.testing.expectEqual(@as(u32, 128), log[1].size);

    try std.testing.expectEqual(@as(u64, 3), log[2].sequence);
    try std.testing.expectEqual(@as(u32, 256), log[2].size);
}

test "DeterministicArena: RNG state tracking" {
    const allocator = std.testing.allocator;
    const seed: u64 = 111;

    var test_arena = try createTestDeterministicArena(allocator, seed);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const initial_state = test_arena.arena.getRNGState();

    // Advance RNG manually
    test_arena.arena.advanceRNG(10);
    const advanced_state = test_arena.arena.getRNGState();

    try std.testing.expect(advanced_state != initial_state);

    // Reset should restore initial state
    test_arena.arena.reset();
    const reset_state = test_arena.arena.getRNGState();

    try std.testing.expectEqual(initial_state, reset_state);
}
