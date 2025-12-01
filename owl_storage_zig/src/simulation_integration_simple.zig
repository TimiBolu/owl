//! Simplified Zig Simulation Integration for Deterministic Testing
//!
//! Task 8.11: Implement Zig simulation integration (Zig)
//!
//! This module provides a simplified but comprehensive simulation integration
//! for the Zig storage layer, focusing on the core requirements:
//! - Simulation-aware memory allocation for deterministic testing
//! - Crash simulation at Zig storage layer
//! - Deterministic object placement for reproducible tests
//! - Simulation-controlled I/O operations

const std = @import("std");
const Allocator = std.mem.Allocator;
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const Arena = @import("arena.zig").Arena;
const DeterministicArena = @import("deterministic_arena.zig").DeterministicArena;
const AllocationStrategy = @import("deterministic_arena.zig").AllocationStrategy;
const GCConfig = @import("deterministic_arena.zig").GCConfig;
const CrashSimulator = @import("crash_simulation.zig").CrashSimulator;
const CrashAwareArena = @import("crash_simulation.zig").CrashAwareArena;
const FailureConfig = @import("crash_simulation.zig").FailureConfig;

/// Simulation mode configuration
pub const SimulationMode = enum {
    /// Production mode - no simulation overhead
    production,
    /// Deterministic mode - reproducible behavior with seed
    deterministic,
    /// Chaos mode - random failures and corruption
    chaos,
    /// Hybrid mode - deterministic with controlled chaos
    hybrid,
};

/// Simulation configuration
pub const SimulationConfig = struct {
    /// Simulation mode
    mode: SimulationMode = .production,
    /// Random seed for deterministic behavior
    seed: u64 = 0,
    /// Allocation strategy for deterministic arena
    allocation_strategy: AllocationStrategy = .bump,
    /// Garbage collection configuration
    gc_config: GCConfig = GCConfig{},
    /// Failure injection configuration
    failure_config: FailureConfig = FailureConfig{},
    /// Enable detailed operation logging
    enable_logging: bool = false,
};

/// Simulation statistics
pub const SimulationStats = struct {
    /// Total operations performed
    total_operations: u64 = 0,
    /// Deterministic allocations performed
    deterministic_allocations: u64 = 0,
    /// Crashes simulated
    crashes_simulated: u64 = 0,
    /// Recovery operations performed
    recovery_operations: u64 = 0,
    /// Simulation time elapsed (virtual nanoseconds)
    simulation_time_ns: u64 = 0,
};

/// Comprehensive simulation-aware storage engine
pub const SimulatedStorageEngine = struct {
    /// Simulation configuration
    config: SimulationConfig,
    /// Deterministic arena (if enabled)
    deterministic_arena: ?DeterministicArena,
    /// Regular arena (fallback)
    regular_arena: ?Arena,
    /// Crash simulator
    crash_simulator: CrashSimulator,
    /// Crash-aware arena wrapper
    crash_aware_arena: ?CrashAwareArena,
    /// Statistics
    stats: SimulationStats,
    /// Allocator for internal operations
    allocator: Allocator,
    /// Current shard ID
    shard_id: u32,
    /// Storage file path
    storage_path: []const u8,

    const Self = @This();

    /// Initialize simulated storage engine
    pub fn init(
        allocator: Allocator,
        config: SimulationConfig,
        shard_id: u32,
        storage_path: []const u8,
        arena_size: usize,
    ) !Self {
        var self = Self{
            .config = config,
            .deterministic_arena = null,
            .regular_arena = null,
            .crash_simulator = CrashSimulator.init(allocator, config.failure_config),
            .crash_aware_arena = null,
            .stats = SimulationStats{},
            .allocator = allocator,
            .shard_id = shard_id,
            .storage_path = try allocator.dupe(u8, storage_path),
        };

        // Initialize appropriate arena based on simulation mode
        switch (config.mode) {
            .production => {
                // Use regular arena for production
                self.regular_arena = try Arena.init(allocator, storage_path, arena_size, shard_id);
            },
            .deterministic, .chaos, .hybrid => {
                // Use deterministic arena for simulation modes
                self.deterministic_arena = try DeterministicArena.init(
                    allocator,
                    storage_path,
                    arena_size,
                    config.seed,
                    config.allocation_strategy,
                    config.gc_config,
                );

                // Enable logging if configured
                if (config.enable_logging) {
                    self.deterministic_arena.?.setLogging(true);
                }
            },
        }

        // Initialize crash-aware wrapper if needed
        if (config.failure_config.crash_enabled or config.failure_config.corruption_enabled) {
            if (self.regular_arena) |*arena| {
                self.crash_aware_arena = CrashAwareArena.init(allocator, arena, &self.crash_simulator);
            }
        }

        return self;
    }

    /// Clean up simulated storage engine
    pub fn deinit(self: *Self) void {
        if (self.deterministic_arena) |*arena| {
            arena.deinit();
        }
        if (self.regular_arena) |*arena| {
            arena.deinit();
        }
        self.crash_simulator.deinit();
        self.allocator.free(self.storage_path);
    }

    /// Reset simulation state for reproducible testing
    pub fn reset(self: *Self) void {
        self.crash_simulator.reset();
        self.stats = SimulationStats{};

        if (self.deterministic_arena) |*arena| {
            arena.reset();
        }
    }

    /// Allocate object with simulation-aware behavior
    pub fn alloc(self: *Self, size: u32) !ObjRef {
        self.stats.total_operations += 1;

        // Check for crash state first
        if (self.crash_simulator.isCrashed()) {
            return error.SystemCrashed;
        }

        if (self.crash_aware_arena) |*crash_arena| {
            return crash_arena.alloc(size);
        } else if (self.deterministic_arena) |*det_arena| {
            self.stats.deterministic_allocations += 1;
            return det_arena.alloc(size);
        } else if (self.regular_arena) |*arena| {
            return arena.alloc(size);
        } else {
            return error.NoArenaInitialized;
        }
    }

    /// Write object with simulation-aware behavior
    pub fn write(self: *Self, objref: ObjRef, data: []const u8) !void {
        self.stats.total_operations += 1;

        // Check for crash state first
        if (self.crash_simulator.isCrashed()) {
            return error.SystemCrashed;
        }

        if (self.crash_aware_arena) |*crash_arena| {
            return crash_arena.write(objref, data);
        } else if (self.deterministic_arena) |*det_arena| {
            return det_arena.write(objref, data);
        } else if (self.regular_arena) |*arena| {
            return arena.write(objref, data);
        } else {
            return error.NoArenaInitialized;
        }
    }

    /// Read object with simulation-aware behavior
    pub fn read(self: *Self, objref: ObjRef, read_allocator: Allocator) !Object {
        self.stats.total_operations += 1;

        // Check for crash state first
        if (self.crash_simulator.isCrashed()) {
            return error.SystemCrashed;
        }

        if (self.crash_aware_arena) |*crash_arena| {
            return crash_arena.read(objref, read_allocator);
        } else if (self.deterministic_arena) |*det_arena| {
            return det_arena.read(objref, read_allocator);
        } else if (self.regular_arena) |*arena| {
            return arena.read(objref, read_allocator);
        } else {
            return error.NoArenaInitialized;
        }
    }

    /// Delete object with simulation-aware behavior
    pub fn delete(self: *Self, objref: ObjRef) !void {
        self.stats.total_operations += 1;

        if (self.crash_aware_arena) |*crash_arena| {
            return crash_arena.delete(objref);
        } else if (self.deterministic_arena) |*det_arena| {
            return det_arena.delete(objref);
        } else if (self.regular_arena) |*arena| {
            return arena.delete(objref);
        } else {
            return error.NoArenaInitialized;
        }
    }

    /// Simulate crash for testing recovery
    pub fn simulateCrash(self: *Self) void {
        self.crash_simulator.crash();
        self.stats.crashes_simulated += 1;
    }

    /// Recover from simulated crash
    pub fn recoverFromCrash(self: *Self) void {
        self.crash_simulator.recover();
        self.stats.recovery_operations += 1;
    }

    /// Check if system is currently crashed
    pub fn isCrashed(self: *const Self) bool {
        return self.crash_simulator.isCrashed();
    }

    /// Get comprehensive statistics
    pub fn getStats(self: *const Self) SimulationEngineStats {
        var arena_stats: ?@import("deterministic_arena.zig").ArenaStats = null;
        if (self.deterministic_arena) |*det_arena| {
            arena_stats = det_arena.stats();
        }

        return SimulationEngineStats{
            .simulation_stats = self.stats,
            .crash_stats = self.crash_simulator.getStats(),
            .arena_stats = arena_stats,
            .shard_id = self.shard_id,
            .mode = self.config.mode,
        };
    }

    /// Update simulation configuration
    pub fn updateConfig(self: *Self, config: SimulationConfig) void {
        self.config = config;
        self.crash_simulator.updateConfig(config.failure_config);

        if (self.deterministic_arena) |*det_arena| {
            det_arena.setStrategy(config.allocation_strategy);
            det_arena.setLogging(config.enable_logging);
        }
    }

    /// Perform deterministic object placement for co-location testing
    pub fn allocNear(self: *Self, size: u32, near: ?ObjRef) !ObjRef {
        self.stats.total_operations += 1;

        if (self.deterministic_arena) |*det_arena| {
            self.stats.deterministic_allocations += 1;
            return det_arena.allocNear(size, near);
        } else if (self.regular_arena) |*arena| {
            return arena.allocNear(size, near);
        } else {
            return error.NoArenaInitialized;
        }
    }

    /// Advance simulation time for time-dependent testing
    pub fn advanceTime(self: *Self, duration_ns: u64) void {
        self.stats.simulation_time_ns += duration_ns;
    }

    /// Get current simulation time
    pub fn getCurrentTime(self: *const Self) u64 {
        return self.stats.simulation_time_ns;
    }
};

/// Combined statistics from all simulation components
pub const SimulationEngineStats = struct {
    simulation_stats: SimulationStats,
    crash_stats: @import("crash_simulation.zig").CrashStats,
    arena_stats: ?@import("deterministic_arena.zig").ArenaStats,
    shard_id: u32,
    mode: SimulationMode,

    /// Generate comprehensive report
    pub fn generateReport(self: *const SimulationEngineStats, allocator: Allocator) ![]u8 {
        var report = std.ArrayList(u8){};
        defer report.deinit(allocator);

        const writer = report.writer(allocator);

        try writer.print("=== Simulation Engine Statistics Report ===\n", .{});
        try writer.print("Shard ID: {d}\n", .{self.shard_id});
        try writer.print("Mode: {s}\n\n", .{@tagName(self.mode)});

        // Simulation statistics
        try writer.print("Simulation Statistics:\n", .{});
        try writer.print("  Total Operations: {d}\n", .{self.simulation_stats.total_operations});
        try writer.print("  Deterministic Allocations: {d}\n", .{self.simulation_stats.deterministic_allocations});
        try writer.print("  Simulation Time: {d} ns\n", .{self.simulation_stats.simulation_time_ns});
        try writer.print("  Crashes Simulated: {d}\n", .{self.simulation_stats.crashes_simulated});
        try writer.print("  Recovery Operations: {d}\n\n", .{self.simulation_stats.recovery_operations});

        // Crash statistics
        try writer.print("Crash Simulation Statistics:\n", .{});
        try writer.print("  Operations Attempted: {d}\n", .{self.crash_stats.operations_attempted});
        try writer.print("  Crashes Simulated: {d}\n", .{self.crash_stats.crashes_simulated});
        try writer.print("  Corruptions Injected: {d}\n", .{self.crash_stats.corruptions_injected});
        try writer.print("  Partial Writes: {d}\n", .{self.crash_stats.partial_writes});
        try writer.print("  Recovery Attempts: {d}\n", .{self.crash_stats.recovery_attempts});
        try writer.print("  Successful Recoveries: {d}\n\n", .{self.crash_stats.successful_recoveries});

        // Arena statistics (if available)
        if (self.arena_stats) |stats| {
            try writer.print("Arena Statistics:\n", .{});
            try writer.print("  Total Size: {d} bytes\n", .{stats.total_size});
            try writer.print("  Used Size: {d} bytes\n", .{stats.used_size});
            try writer.print("  Free Size: {d} bytes\n", .{stats.free_size});
            try writer.print("  Object Count: {d}\n", .{stats.object_count});
            try writer.print("  Allocation Sequence: {d}\n", .{stats.allocation_sequence});
            try writer.print("  Current Strategy: {s}\n", .{@tagName(stats.current_strategy)});
            try writer.print("  RNG State: {d}\n", .{stats.rng_state});
        }

        return report.toOwnedSlice(allocator);
    }
};

// ============================================================================
// Tests
// ============================================================================

/// Helper function to create test storage engine
fn createTestEngine(allocator: std.mem.Allocator, mode: SimulationMode, seed: u64) !struct { engine: SimulatedStorageEngine, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_sim_simple.dat" });
    defer allocator.free(file_path);

    const config = SimulationConfig{
        .mode = mode,
        .seed = seed,
        .allocation_strategy = .bump,
        .enable_logging = true,
        .failure_config = FailureConfig{
            .crash_enabled = (mode != .production),
            .crash_mode = if (mode == .chaos) .random_timing else .before_write,
            .crash_probability = if (mode == .chaos) 0.3 else 0.1,
            .corruption_enabled = (mode == .chaos),
            .seed = seed,
        },
    };

    const engine = try SimulatedStorageEngine.init(allocator, config, 0, file_path, 1024 * 1024);
    return .{ .engine = engine, .dir = tmp };
}

test "SimulatedStorageEngine: deterministic allocation patterns" {
    const allocator = std.testing.allocator;
    const seed: u64 = 123;

    // Create two engines with same seed
    var test_engine1 = try createTestEngine(allocator, .deterministic, seed);
    defer test_engine1.dir.cleanup();
    defer test_engine1.engine.deinit();

    var test_engine2 = try createTestEngine(allocator, .deterministic, seed);
    defer test_engine2.dir.cleanup();
    defer test_engine2.engine.deinit();

    // Perform same allocations on both engines
    const sizes = [_]u32{ 64, 128, 256, 512 };
    var offsets1: [4]u64 = undefined;
    var offsets2: [4]u64 = undefined;

    for (sizes, 0..) |size, i| {
        const ref1 = try test_engine1.engine.alloc(size);
        const ref2 = try test_engine2.engine.alloc(size);

        offsets1[i] = ref1.getOffset();
        offsets2[i] = ref2.getOffset();
    }

    // Verify identical allocation patterns
    for (offsets1, offsets2) |o1, o2| {
        try std.testing.expectEqual(o1, o2);
    }

    // Verify statistics match
    const stats1 = test_engine1.engine.getStats();
    const stats2 = test_engine2.engine.getStats();

    try std.testing.expectEqual(stats1.simulation_stats.total_operations, stats2.simulation_stats.total_operations);
    try std.testing.expectEqual(stats1.simulation_stats.deterministic_allocations, stats2.simulation_stats.deterministic_allocations);
}

test "SimulatedStorageEngine: crash simulation and recovery" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestEngine(allocator, .chaos, 456);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Verify initial state
    try std.testing.expect(!test_engine.engine.isCrashed());

    // Simulate crash
    test_engine.engine.simulateCrash();
    try std.testing.expect(test_engine.engine.isCrashed());

    // Operations should fail while crashed
    const result = test_engine.engine.alloc(64);
    try std.testing.expectError(error.SystemCrashed, result);

    // Recover from crash
    test_engine.engine.recoverFromCrash();
    try std.testing.expect(!test_engine.engine.isCrashed());

    // Operations should work after recovery
    const ref = try test_engine.engine.alloc(64);
    try std.testing.expect(!ref.isNull());

    // Verify crash statistics
    const stats = test_engine.engine.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.simulation_stats.crashes_simulated);
    try std.testing.expectEqual(@as(u64, 1), stats.simulation_stats.recovery_operations);
}

test "SimulatedStorageEngine: object storage and retrieval" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestEngine(allocator, .deterministic, 789);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Create and store an object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 42, id, &[_]u8{ 1, 2, 3 }, "test data", &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const ref = try test_engine.engine.alloc(@intCast(data.len));
    try test_engine.engine.write(ref, data);

    // Read object back
    const read_obj = try test_engine.engine.read(ref, allocator);
    var read_obj_mut = read_obj;
    defer read_obj_mut.deinit();

    // Verify object integrity
    try std.testing.expectEqual(obj.header.type_tag, read_obj.header.type_tag);
    try std.testing.expectEqualSlices(u8, &obj.header.id, &read_obj.header.id);
    try std.testing.expectEqualSlices(u8, obj.fixed, read_obj.fixed);
    try std.testing.expectEqualSlices(u8, obj.var_data, read_obj.var_data);
}

test "SimulatedStorageEngine: reset reproducibility" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestEngine(allocator, .deterministic, 222);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // First run
    const ref1_run1 = try test_engine.engine.alloc(64);
    const ref2_run1 = try test_engine.engine.alloc(128);
    const stats1 = test_engine.engine.getStats();

    // Reset and second run
    test_engine.engine.reset();
    const ref1_run2 = try test_engine.engine.alloc(64);
    const ref2_run2 = try test_engine.engine.alloc(128);
    const stats2 = test_engine.engine.getStats();

    // Should produce identical results
    try std.testing.expectEqual(ref1_run1.getOffset(), ref1_run2.getOffset());
    try std.testing.expectEqual(ref2_run1.getOffset(), ref2_run2.getOffset());
    try std.testing.expectEqual(stats1.simulation_stats.deterministic_allocations, stats2.simulation_stats.deterministic_allocations);
}

test "SimulatedStorageEngine: time simulation" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestEngine(allocator, .deterministic, 333);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    const initial_time = test_engine.engine.getCurrentTime();

    // Manually advance time
    test_engine.engine.advanceTime(1000000); // 1ms
    const time_after_advance = test_engine.engine.getCurrentTime();
    try std.testing.expectEqual(initial_time + 1000000, time_after_advance);
}

test "SimulatedStorageEngine: comprehensive statistics" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestEngine(allocator, .hybrid, 777);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Perform various operations
    _ = try test_engine.engine.alloc(64);
    _ = try test_engine.engine.alloc(128);
    test_engine.engine.simulateCrash();
    test_engine.engine.recoverFromCrash();
    test_engine.engine.advanceTime(1000000);

    // Get comprehensive statistics
    const stats = test_engine.engine.getStats();

    // Verify all statistics are populated
    try std.testing.expect(stats.simulation_stats.total_operations > 0);
    try std.testing.expect(stats.simulation_stats.deterministic_allocations > 0);
    try std.testing.expect(stats.simulation_stats.simulation_time_ns > 0);
    try std.testing.expectEqual(@as(u64, 1), stats.simulation_stats.crashes_simulated);
    try std.testing.expectEqual(@as(u64, 1), stats.simulation_stats.recovery_operations);
    try std.testing.expectEqual(@as(u32, 0), stats.shard_id);
    try std.testing.expectEqual(SimulationMode.hybrid, stats.mode);

    // Generate and verify report
    const report = try stats.generateReport(allocator);
    defer allocator.free(report);

    try std.testing.expect(std.mem.indexOf(u8, report, "Simulation Engine Statistics Report") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "Shard ID: 0") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "Mode: hybrid") != null);
}
