//! Zig Simulation Integration for Deterministic Testing
//!
//! Task 8.11: Implement Zig simulation integration (Zig)
//!
//! This module provides comprehensive simulation integration for the Zig storage layer,
//! combining deterministic allocation, crash simulation, and I/O control into a unified
//! simulation framework. It enables reproducible testing of complex distributed scenarios
//! at the storage layer level.
//!
//! Features:
//! - Simulation-aware memory allocation for deterministic testing
//! - Crash simulation at Zig storage layer
//! - Deterministic object placement for reproducible tests
//! - Simulation-controlled I/O operations
//! - Integration with Arena, WAL, and distributed components

const std = @import("std");
const Allocator = std.mem.Allocator;
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const Arena = @import("arena.zig").Arena;
const DeterministicArena = @import("deterministic_arena.zig").DeterministicArena;
const AllocationStrategy = @import("deterministic_arena.zig").AllocationStrategy;
const GCConfig = @import("deterministic_arena.zig").GCConfig;
const CrashSimulator = @import("crash_simulation.zig").CrashSimulator;
const CrashAwareArena = @import("crash_simulation.zig").CrashAwareArena;
const FailureConfig = @import("crash_simulation.zig").FailureConfig;
const CrashMode = @import("crash_simulation.zig").CrashMode;
const CorruptionType = @import("crash_simulation.zig").CorruptionType;
const WAL = @import("wal.zig").WAL;
const WALEntry = @import("wal.zig").WALEntry;
const LSN = @import("wal.zig").LSN;

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

/// I/O operation types for simulation control
pub const IOOperation = enum {
    /// Memory allocation operations
    memory_alloc,
    memory_free,
    memory_read,
    memory_write,
    /// File system operations
    file_open,
    file_read,
    file_write,
    file_sync,
    file_close,
    /// WAL operations
    wal_append,
    wal_sync,
    wal_checkpoint,
    /// Network operations (for distributed components)
    network_send,
    network_receive,
    network_connect,
};

/// Simulation statistics for analysis
pub const SimulationStats = struct {
    /// Total operations performed
    total_operations: u64 = 0,
    /// Operations by type
    operations_by_type: std.EnumMap(IOOperation, u64) = std.EnumMap(IOOperation, u64).init(.{}),
    /// Failures injected by type
    failures_by_type: std.EnumMap(IOOperation, u64) = std.EnumMap(IOOperation, u64).init(.{}),
    /// Deterministic allocations performed
    deterministic_allocations: u64 = 0,
    /// Crash simulations triggered
    crashes_simulated: u64 = 0,
    /// Corruptions injected
    corruptions_injected: u64 = 0,
    /// Recovery operations performed
    recovery_operations: u64 = 0,
    /// Simulation time elapsed (virtual)
    simulation_time_ns: u64 = 0,
    /// Real time elapsed
    real_time_ns: u64 = 0,

    /// Calculate simulation speedup factor
    pub fn speedupFactor(self: *const SimulationStats) f64 {
        if (self.real_time_ns == 0) return 0.0;
        return @as(f64, @floatFromInt(self.simulation_time_ns)) / @as(f64, @floatFromInt(self.real_time_ns));
    }

    /// Get failure rate for specific operation type
    pub fn failureRate(self: *const SimulationStats, op_type: IOOperation) f64 {
        const total = self.operations_by_type.get(op_type);
        const failures = self.failures_by_type.get(op_type);
        if (total == 0) return 0.0;
        return @as(f64, @floatFromInt(failures)) / @as(f64, @floatFromInt(total));
    }
};

/// Simulation configuration combining all aspects
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
    /// Maximum log entries to keep
    max_log_entries: u32 = 10000,
    /// Simulation time acceleration factor
    time_acceleration: f64 = 1000.0, // 1000x faster than real time
    /// Enable I/O operation simulation
    simulate_io: bool = true,
    /// Enable network operation simulation
    simulate_network: bool = true,
    /// Enable memory operation simulation
    simulate_memory: bool = true,
};

/// Simulation event for logging and analysis
pub const SimulationEvent = struct {
    /// Event sequence number
    sequence: u64,
    /// Simulation timestamp
    timestamp_ns: u64,
    /// Operation type
    operation: IOOperation,
    /// Operation parameters (encoded as string for simplicity)
    parameters: []const u8,
    /// Operation result (success/failure)
    success: bool,
    /// Error message if failed
    error_message: ?[]const u8,
    /// Duration in nanoseconds
    duration_ns: u64,
    /// Random seed state at time of operation
    rng_state: u64,
};

/// Deterministic Random Number Generator for simulation
pub const SimulationRNG = struct {
    state: u64,

    const MULTIPLIER: u64 = 1103515245;
    const INCREMENT: u64 = 12345;
    const MODULUS: u64 = 1 << 31;

    pub fn init(seed: u64) SimulationRNG {
        return .{ .state = seed };
    }

    pub fn next(self: *SimulationRNG) u32 {
        self.state = (MULTIPLIER *% self.state +% INCREMENT) % MODULUS;
        return @intCast(self.state);
    }

    pub fn nextFloat(self: *SimulationRNG) f64 {
        return @as(f64, @floatFromInt(self.next())) / @as(f64, @floatFromInt(MODULUS));
    }

    pub fn nextRange(self: *SimulationRNG, max: u32) u32 {
        if (max == 0) return 0;
        return self.next() % max;
    }

    pub fn nextBool(self: *SimulationRNG) bool {
        return (self.next() & 1) == 1;
    }

    pub fn reset(self: *SimulationRNG, seed: u64) void {
        self.state = seed;
    }
};

/// Simulated I/O interface for controlled operations
pub const SimulatedIO = struct {
    /// Configuration
    config: SimulationConfig,
    /// Random number generator
    rng: SimulationRNG,
    /// Statistics tracking
    stats: SimulationStats,
    /// Event log
    event_log: std.ArrayList(SimulationEvent),
    /// Current simulation time
    simulation_time_ns: u64,
    /// Real start time for performance measurement
    real_start_time: i128,
    /// Allocator for internal operations
    allocator: Allocator,
    /// Operation sequence counter
    operation_sequence: u64,

    const Self = @This();

    /// Initialize simulated I/O
    pub fn init(allocator: Allocator, config: SimulationConfig) Self {
        return .{
            .config = config,
            .rng = SimulationRNG.init(config.seed),
            .stats = SimulationStats{},
            .event_log = .{},
            .simulation_time_ns = 0,
            .real_start_time = std.time.nanoTimestamp(),
            .allocator = allocator,
            .operation_sequence = 0,
        };
    }

    /// Clean up simulated I/O
    pub fn deinit(self: *Self) void {
        // Free event log strings
        for (self.event_log.items) |event| {
            self.allocator.free(event.parameters);
            if (event.error_message) |msg| {
                self.allocator.free(msg);
            }
        }
        self.event_log.deinit(self.allocator);
    }

    /// Reset simulation state
    pub fn reset(self: *Self) void {
        self.rng.reset(self.config.seed);
        self.stats = SimulationStats{};
        self.simulation_time_ns = 0;
        self.real_start_time = std.time.nanoTimestamp();
        self.operation_sequence = 0;

        // Clear event log
        for (self.event_log.items) |event| {
            self.allocator.free(event.parameters);
            if (event.error_message) |msg| {
                self.allocator.free(msg);
            }
        }
        self.event_log.clearRetainingCapacity();
    }

    /// Simulate an I/O operation with potential failure injection
    pub fn simulateOperation(
        self: *Self,
        operation: IOOperation,
        parameters: []const u8,
        real_operation: anytype,
    ) @TypeOf(real_operation()) {
        self.operation_sequence += 1;
        const start_time = std.time.nanoTimestamp();
        const start_sim_time = self.simulation_time_ns;

        // Update statistics
        self.stats.total_operations += 1;
        const current_count = self.stats.operations_by_type.get(operation);
        self.stats.operations_by_type.put(operation, current_count + 1);

        // Check for failure injection
        const should_fail = self.shouldInjectFailure(operation);
        var success = true;
        var error_message: ?[]const u8 = null;

        if (should_fail) {
            // Inject failure
            const failure_count = self.stats.failures_by_type.get(operation);
            self.stats.failures_by_type.put(operation, failure_count + 1);
            success = false;
            error_message = self.allocator.dupe(u8, "Simulated failure") catch null;

            // Calculate timing for failed operation
            const end_time = std.time.nanoTimestamp();
            const real_duration = @as(u64, @intCast(end_time - start_time));
            const sim_duration = @as(u64, @intFromFloat(@as(f64, @floatFromInt(real_duration)) * self.config.time_acceleration));

            // Advance simulation time
            self.simulation_time_ns += sim_duration;
            self.stats.simulation_time_ns = self.simulation_time_ns;
            self.stats.real_time_ns = @intCast(end_time - self.real_start_time);

            // Log event if enabled
            if (self.config.enable_logging and self.event_log.items.len < self.config.max_log_entries) {
                const event = SimulationEvent{
                    .sequence = self.operation_sequence,
                    .timestamp_ns = start_sim_time,
                    .operation = operation,
                    .parameters = self.allocator.dupe(u8, parameters) catch return error.SimulatedFailure,
                    .success = success,
                    .error_message = error_message,
                    .duration_ns = sim_duration,
                    .rng_state = self.rng.state,
                };
                self.event_log.append(self.allocator, event) catch {};
            }

            return error.SimulatedFailure;
        }

        // Perform real operation
        const result = real_operation() catch |err| {
            success = false;
            error_message = self.allocator.dupe(u8, @errorName(err)) catch null;

            // Calculate timing for failed operation
            const end_time = std.time.nanoTimestamp();
            const real_duration = @as(u64, @intCast(end_time - start_time));
            const sim_duration = @as(u64, @intFromFloat(@as(f64, @floatFromInt(real_duration)) * self.config.time_acceleration));

            // Advance simulation time
            self.simulation_time_ns += sim_duration;
            self.stats.simulation_time_ns = self.simulation_time_ns;
            self.stats.real_time_ns = @intCast(end_time - self.real_start_time);

            // Log event if enabled
            if (self.config.enable_logging and self.event_log.items.len < self.config.max_log_entries) {
                const event = SimulationEvent{
                    .sequence = self.operation_sequence,
                    .timestamp_ns = start_sim_time,
                    .operation = operation,
                    .parameters = self.allocator.dupe(u8, parameters) catch return err,
                    .success = success,
                    .error_message = error_message,
                    .duration_ns = sim_duration,
                    .rng_state = self.rng.state,
                };
                self.event_log.append(self.allocator, event) catch {};
            }

            return err;
        };

        // Calculate timing
        const end_time = std.time.nanoTimestamp();
        const real_duration = @as(u64, @intCast(end_time - start_time));
        const sim_duration = @as(u64, @intFromFloat(@as(f64, @floatFromInt(real_duration)) * self.config.time_acceleration));

        // Advance simulation time
        self.simulation_time_ns += sim_duration;
        self.stats.simulation_time_ns = self.simulation_time_ns;
        self.stats.real_time_ns = @intCast(end_time - self.real_start_time);

        // Log event if enabled
        if (self.config.enable_logging and self.event_log.items.len < self.config.max_log_entries) {
            const event = SimulationEvent{
                .sequence = self.operation_sequence,
                .timestamp_ns = start_sim_time,
                .operation = operation,
                .parameters = try self.allocator.dupe(u8, parameters),
                .success = success,
                .error_message = error_message,
                .duration_ns = sim_duration,
                .rng_state = self.rng.state,
            };
            self.event_log.append(self.allocator, event) catch {};
        }

        return result;
    }

    /// Check if failure should be injected for operation type
    fn shouldInjectFailure(self: *Self, operation: IOOperation) bool {
        if (self.config.mode == .production) return false;

        // Different failure probabilities based on operation type
        const base_probability = switch (operation) {
            .memory_alloc, .memory_free => 0.001, // Very low for memory ops
            .file_read, .file_write => 0.01, // Low for file ops
            .file_sync, .wal_sync => 0.02, // Higher for sync ops
            .network_send, .network_receive => 0.05, // Higher for network
            else => 0.005, // Default low probability
        };

        const mode_multiplier = switch (self.config.mode) {
            .production => 0.0,
            .deterministic => 1.0,
            .chaos => 10.0, // Much higher failure rate
            .hybrid => 3.0, // Moderate increase
        };

        const final_probability = base_probability * mode_multiplier;
        return self.rng.nextFloat() < final_probability;
    }

    /// Get current simulation statistics
    pub fn getStats(self: *const Self) SimulationStats {
        return self.stats;
    }

    /// Get event log for analysis
    pub fn getEventLog(self: *const Self) []const SimulationEvent {
        return self.event_log.items;
    }

    /// Advance simulation time manually
    pub fn advanceTime(self: *Self, duration_ns: u64) void {
        self.simulation_time_ns += duration_ns;
        self.stats.simulation_time_ns = self.simulation_time_ns;
    }

    /// Get current simulation time
    pub fn getCurrentTime(self: *const Self) u64 {
        return self.simulation_time_ns;
    }
};

/// Comprehensive simulation-aware storage engine
pub const SimulatedStorageEngine = struct {
    /// Simulation configuration
    config: SimulationConfig,
    /// Simulated I/O controller
    simulated_io: SimulatedIO,
    /// Deterministic arena (if enabled)
    deterministic_arena: ?DeterministicArena,
    /// Regular arena (fallback)
    regular_arena: ?Arena,
    /// Crash simulator
    crash_simulator: CrashSimulator,
    /// Crash-aware arena wrapper
    crash_aware_arena: ?CrashAwareArena,
    /// WAL instance
    wal: ?WAL,
    /// Allocator for internal operations
    allocator: Allocator,
    /// Current shard ID
    shard_id: ShardId,
    /// Storage file path
    storage_path: []const u8,

    const Self = @This();

    /// Initialize simulated storage engine
    pub fn init(
        allocator: Allocator,
        config: SimulationConfig,
        shard_id: ShardId,
        storage_path: []const u8,
        arena_size: usize,
    ) !Self {
        var self = Self{
            .config = config,
            .simulated_io = SimulatedIO.init(allocator, config),
            .deterministic_arena = null,
            .regular_arena = null,
            .crash_simulator = CrashSimulator.init(allocator, config.failure_config),
            .crash_aware_arena = null,
            .wal = null,
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
        if (self.wal) |*wal| {
            wal.deinit();
        }
        self.crash_simulator.deinit();
        self.simulated_io.deinit();
        self.allocator.free(self.storage_path);
    }

    /// Reset simulation state for reproducible testing
    pub fn reset(self: *Self) void {
        self.simulated_io.reset();
        self.crash_simulator.reset();

        if (self.deterministic_arena) |*arena| {
            arena.reset();
        }
    }

    /// Allocate object with simulation-aware behavior
    pub fn alloc(self: *Self, size: u32) !ObjRef {
        const params = try std.fmt.allocPrint(self.allocator, "size={d}", .{size});
        defer self.allocator.free(params);

        return self.simulated_io.simulateOperation(.memory_alloc, params, struct {
            fn operation(engine: *Self, obj_size: u32) !ObjRef {
                if (engine.crash_aware_arena) |*crash_arena| {
                    return crash_arena.alloc(obj_size);
                } else if (engine.deterministic_arena) |*det_arena| {
                    engine.simulated_io.stats.deterministic_allocations += 1;
                    return det_arena.alloc(obj_size);
                } else if (engine.regular_arena) |*arena| {
                    return arena.alloc(obj_size);
                } else {
                    return error.NoArenaInitialized;
                }
            }
        }.operation(self, size));
    }

    /// Write object with simulation-aware behavior
    pub fn write(self: *Self, objref: ObjRef, data: []const u8) !void {
        const params = try std.fmt.allocPrint(self.allocator, "objref={d},size={d}", .{ objref.getOffset(), data.len });
        defer self.allocator.free(params);

        return self.simulated_io.simulateOperation(.memory_write, params, struct {
            fn operation(engine: *Self, obj_ref: ObjRef, obj_data: []const u8) !void {
                if (engine.crash_aware_arena) |*crash_arena| {
                    return crash_arena.write(obj_ref, obj_data);
                } else if (engine.deterministic_arena) |*det_arena| {
                    return det_arena.write(obj_ref, obj_data);
                } else if (engine.regular_arena) |*arena| {
                    return arena.write(obj_ref, obj_data);
                } else {
                    return error.NoArenaInitialized;
                }
            }
        }.operation(self, objref, data));
    }

    /// Read object with simulation-aware behavior
    pub fn read(self: *Self, objref: ObjRef, read_allocator: Allocator) !Object {
        const params = try std.fmt.allocPrint(self.allocator, "objref={d}", .{objref.getOffset()});
        defer self.allocator.free(params);

        return self.simulated_io.simulateOperation(.memory_read, params, struct {
            fn operation(engine: *Self, obj_ref: ObjRef, read_alloc: Allocator) !Object {
                if (engine.crash_aware_arena) |*crash_arena| {
                    return crash_arena.read(obj_ref, read_alloc);
                } else if (engine.deterministic_arena) |*det_arena| {
                    return det_arena.read(obj_ref, read_alloc);
                } else if (engine.regular_arena) |*arena| {
                    return arena.read(obj_ref, read_alloc);
                } else {
                    return error.NoArenaInitialized;
                }
            }
        }.operation(self, objref, read_allocator));
    }

    /// Delete object with simulation-aware behavior
    pub fn delete(self: *Self, objref: ObjRef) !void {
        const params = try std.fmt.allocPrint(self.allocator, "objref={d}", .{objref.getOffset()});
        defer self.allocator.free(params);

        return self.simulated_io.simulateOperation(.memory_free, params, struct {
            fn operation(engine: *Self, obj_ref: ObjRef) !void {
                if (engine.crash_aware_arena) |*crash_arena| {
                    return crash_arena.delete(obj_ref);
                } else if (engine.deterministic_arena) |*det_arena| {
                    return det_arena.delete(obj_ref);
                } else if (engine.regular_arena) |*arena| {
                    return arena.delete(obj_ref);
                } else {
                    return error.NoArenaInitialized;
                }
            }
        }.operation(self, objref));
    }

    /// Simulate crash for testing recovery
    pub fn simulateCrash(self: *Self) void {
        self.crash_simulator.crash();
        self.simulated_io.stats.crashes_simulated += 1;
    }

    /// Recover from simulated crash
    pub fn recoverFromCrash(self: *Self) void {
        self.crash_simulator.recover();
        self.simulated_io.stats.recovery_operations += 1;
    }

    /// Check if system is currently crashed
    pub fn isCrashed(self: *const Self) bool {
        return self.crash_simulator.isCrashed();
    }

    /// Get comprehensive statistics
    pub fn getStats(self: *const Self) SimulationEngineStats {
        var arena_stats: ?DeterministicArena.ArenaStats = null;
        if (self.deterministic_arena) |*det_arena| {
            arena_stats = det_arena.stats();
        }

        return SimulationEngineStats{
            .simulation_stats = self.simulated_io.getStats(),
            .crash_stats = self.crash_simulator.getStats(),
            .arena_stats = arena_stats,
            .shard_id = self.shard_id,
            .mode = self.config.mode,
        };
    }

    /// Get event log for analysis
    pub fn getEventLog(self: *const Self) []const SimulationEvent {
        return self.simulated_io.getEventLog();
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
        const params = try std.fmt.allocPrint(self.allocator, "size={d},near={?d}", .{ size, if (near) |ref| ref.getOffset() else null });
        defer self.allocator.free(params);

        return self.simulated_io.simulateOperation(.memory_alloc, params, struct {
            fn operation(engine: *Self, obj_size: u32, near_ref: ?ObjRef) !ObjRef {
                if (engine.deterministic_arena) |*det_arena| {
                    engine.simulated_io.stats.deterministic_allocations += 1;
                    return det_arena.allocNear(obj_size, near_ref);
                } else if (engine.regular_arena) |*arena| {
                    return arena.allocNear(obj_size, near_ref);
                } else {
                    return error.NoArenaInitialized;
                }
            }
        }.operation(self, size, near));
    }

    /// Advance simulation time for time-dependent testing
    pub fn advanceTime(self: *Self, duration_ns: u64) void {
        self.simulated_io.advanceTime(duration_ns);
    }

    /// Get current simulation time
    pub fn getCurrentTime(self: *const Self) u64 {
        return self.simulated_io.getCurrentTime();
    }
};

/// Combined statistics from all simulation components
pub const SimulationEngineStats = struct {
    simulation_stats: SimulationStats,
    crash_stats: @import("crash_simulation.zig").CrashStats,
    arena_stats: ?DeterministicArena.ArenaStats,
    shard_id: ShardId,
    mode: SimulationMode,

    /// Generate comprehensive report
    pub fn generateReport(self: *const SimulationEngineStats, allocator: Allocator) ![]u8 {
        var report = std.ArrayList(u8).init(allocator);
        defer report.deinit();

        const writer = report.writer();

        try writer.print("=== Simulation Engine Statistics Report ===\n");
        try writer.print("Shard ID: {d}\n", .{self.shard_id});
        try writer.print("Mode: {s}\n\n", .{@tagName(self.mode)});

        // Simulation statistics
        try writer.print("Simulation Statistics:\n");
        try writer.print("  Total Operations: {d}\n", .{self.simulation_stats.total_operations});
        try writer.print("  Deterministic Allocations: {d}\n", .{self.simulation_stats.deterministic_allocations});
        try writer.print("  Simulation Time: {d} ns\n", .{self.simulation_stats.simulation_time_ns});
        try writer.print("  Real Time: {d} ns\n", .{self.simulation_stats.real_time_ns});
        try writer.print("  Speedup Factor: {d:.2}x\n\n", .{self.simulation_stats.speedupFactor()});

        // Crash statistics
        try writer.print("Crash Simulation Statistics:\n");
        try writer.print("  Operations Attempted: {d}\n", .{self.crash_stats.operations_attempted});
        try writer.print("  Crashes Simulated: {d}\n", .{self.crash_stats.crashes_simulated});
        try writer.print("  Corruptions Injected: {d}\n", .{self.crash_stats.corruptions_injected});
        try writer.print("  Partial Writes: {d}\n", .{self.crash_stats.partial_writes});
        try writer.print("  Recovery Attempts: {d}\n", .{self.crash_stats.recovery_attempts});
        try writer.print("  Successful Recoveries: {d}\n\n", .{self.crash_stats.successful_recoveries});

        // Arena statistics (if available)
        if (self.arena_stats) |stats| {
            try writer.print("Arena Statistics:\n");
            try writer.print("  Total Size: {d} bytes\n", .{stats.total_size});
            try writer.print("  Used Size: {d} bytes\n", .{stats.used_size});
            try writer.print("  Free Size: {d} bytes\n", .{stats.free_size});
            try writer.print("  Object Count: {d}\n", .{stats.object_count});
            try writer.print("  Allocation Sequence: {d}\n", .{stats.allocation_sequence});
            try writer.print("  Current Strategy: {s}\n", .{@tagName(stats.current_strategy)});
            try writer.print("  RNG State: {d}\n", .{stats.rng_state});
        }

        return report.toOwnedSlice();
    }
};

// ============================================================================
// Tests
// ============================================================================

test "SimulationRNG: deterministic behavior" {
    var rng1 = SimulationRNG.init(12345);
    var rng2 = SimulationRNG.init(12345);

    // Same seed should produce same sequence
    for (0..100) |_| {
        try std.testing.expectEqual(rng1.next(), rng2.next());
    }

    // Different seeds should produce different sequences
    var rng3 = SimulationRNG.init(54321);
    rng1.reset(12345);

    var different_count: u32 = 0;
    for (0..100) |_| {
        if (rng1.next() != rng3.next()) {
            different_count += 1;
        }
    }

    try std.testing.expect(different_count > 80);
}

test "SimulatedIO: operation simulation" {
    const allocator = std.testing.allocator;

    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 42,
        .enable_logging = true,
        .simulate_io = true,
    };

    var sim_io = SimulatedIO.init(allocator, config);
    defer sim_io.deinit();

    // Simulate some operations
    var counter: u32 = 0;
    const result1 = try sim_io.simulateOperation(.memory_alloc, "size=64", struct {
        fn op(cnt: *u32) !u32 {
            cnt.* += 1;
            return cnt.*;
        }
    }.op(&counter));

    try std.testing.expectEqual(@as(u32, 1), result1);

    const result2 = try sim_io.simulateOperation(.memory_write, "offset=100", struct {
        fn op(cnt: *u32) !u32 {
            cnt.* += 10;
            return cnt.*;
        }
    }.op(&counter));

    try std.testing.expectEqual(@as(u32, 11), result2);

    // Check statistics
    const stats = sim_io.getStats();
    try std.testing.expectEqual(@as(u64, 2), stats.total_operations);
    try std.testing.expectEqual(@as(u64, 1), stats.operations_by_type.get(.memory_alloc));
    try std.testing.expectEqual(@as(u64, 1), stats.operations_by_type.get(.memory_write));

    // Check event log
    const events = sim_io.getEventLog();
    try std.testing.expectEqual(@as(usize, 2), events.len);
    try std.testing.expectEqualStrings("size=64", events[0].parameters);
    try std.testing.expectEqualStrings("offset=100", events[1].parameters);
}

fn createTestSimulatedEngine(allocator: std.mem.Allocator, mode: SimulationMode) !struct { engine: SimulatedStorageEngine, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_sim_engine.dat" });
    defer allocator.free(file_path);

    const config = SimulationConfig{
        .mode = mode,
        .seed = 123,
        .enable_logging = true,
        .allocation_strategy = .bump,
        .failure_config = FailureConfig{
            .crash_enabled = (mode != .production),
            .corruption_enabled = (mode == .chaos),
            .seed = 123,
        },
    };

    const engine = try SimulatedStorageEngine.init(allocator, config, 0, file_path, 1024 * 1024);
    return .{ .engine = engine, .dir = tmp };
}

test "SimulatedStorageEngine: deterministic mode" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestSimulatedEngine(allocator, .deterministic);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Perform some operations
    const ref1 = try test_engine.engine.alloc(64);
    const ref2 = try test_engine.engine.alloc(128);

    try std.testing.expect(!ref1.isNull());
    try std.testing.expect(!ref2.isNull());
    try std.testing.expect(ref1.getOffset() != ref2.getOffset());

    // Create and write an object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, "test data", &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    try test_engine.engine.write(ref1, data);

    // Read it back
    const read_obj = try test_engine.engine.read(ref1, allocator);
    var read_obj_mut = read_obj;
    defer read_obj_mut.deinit();

    try std.testing.expectEqual(obj.type_tag, read_obj.type_tag);
    try std.testing.expectEqualSlices(u8, &obj.id, &read_obj.id);

    // Check statistics
    const stats = test_engine.engine.getStats();
    try std.testing.expect(stats.simulation_stats.total_operations > 0);
    try std.testing.expect(stats.simulation_stats.deterministic_allocations > 0);
}

test "SimulatedStorageEngine: crash simulation" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestSimulatedEngine(allocator, .chaos);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Simulate crash
    test_engine.engine.simulateCrash();
    try std.testing.expect(test_engine.engine.isCrashed());

    // Operations should fail while crashed
    const result = test_engine.engine.alloc(64);
    try std.testing.expectError(error.SystemCrashed, result);

    // Recover and try again
    test_engine.engine.recoverFromCrash();
    try std.testing.expect(!test_engine.engine.isCrashed());

    const ref = try test_engine.engine.alloc(64);
    try std.testing.expect(!ref.isNull());

    // Check crash statistics
    const stats = test_engine.engine.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.simulation_stats.crashes_simulated);
    try std.testing.expectEqual(@as(u64, 1), stats.simulation_stats.recovery_operations);
}

test "SimulatedStorageEngine: reset reproducibility" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestSimulatedEngine(allocator, .deterministic);
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

test "SimulatedStorageEngine: co-location testing" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestSimulatedEngine(allocator, .deterministic);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Update to use co-location strategy
    var config = test_engine.engine.config;
    config.allocation_strategy = .deterministic_colocation;
    test_engine.engine.updateConfig(config);

    // Allocate base object
    const base_ref = try test_engine.engine.alloc(64);

    // Allocate near base object
    const near_ref = try test_engine.engine.allocNear(64, base_ref);

    try std.testing.expect(!base_ref.isNull());
    try std.testing.expect(!near_ref.isNull());

    // Objects should be allocated (exact co-location depends on implementation)
    const stats = test_engine.engine.getStats();
    try std.testing.expect(stats.simulation_stats.deterministic_allocations >= 2);
}

test "SimulatedStorageEngine: time simulation" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestSimulatedEngine(allocator, .deterministic);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    const initial_time = test_engine.engine.getCurrentTime();

    // Perform some operations (should advance time)
    _ = try test_engine.engine.alloc(64);
    _ = try test_engine.engine.alloc(128);

    const time_after_ops = test_engine.engine.getCurrentTime();
    try std.testing.expect(time_after_ops > initial_time);

    // Manually advance time
    test_engine.engine.advanceTime(1000000); // 1ms
    const time_after_advance = test_engine.engine.getCurrentTime();
    try std.testing.expectEqual(time_after_ops + 1000000, time_after_advance);
}

test "SimulationEngineStats: report generation" {
    const allocator = std.testing.allocator;

    var test_engine = try createTestSimulatedEngine(allocator, .deterministic);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Perform some operations to generate statistics
    _ = try test_engine.engine.alloc(64);
    _ = try test_engine.engine.alloc(128);
    test_engine.engine.simulateCrash();
    test_engine.engine.recoverFromCrash();

    const stats = test_engine.engine.getStats();
    const report = try stats.generateReport(allocator);
    defer allocator.free(report);

    // Verify report contains expected sections
    try std.testing.expect(std.mem.indexOf(u8, report, "Simulation Engine Statistics Report") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "Simulation Statistics:") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "Crash Simulation Statistics:") != null);
    try std.testing.expect(std.mem.indexOf(u8, report, "Arena Statistics:") != null);
}
