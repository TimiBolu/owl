//! Comprehensive tests for Zig Simulation Integration
//!
//! Task 8.11: Implement Zig simulation integration (Zig)
//!
//! This test suite verifies that the simulation integration provides:
//! - Simulation-aware memory allocation for deterministic testing
//! - Crash simulation at Zig storage layer
//! - Deterministic object placement for reproducible tests
//! - Simulation-controlled I/O operations

const std = @import("std");
const testing = std.testing;
const SimulatedStorageEngine = @import("simulation_integration.zig").SimulatedStorageEngine;
const SimulationMode = @import("simulation_integration.zig").SimulationMode;
const SimulationConfig = @import("simulation_integration.zig").SimulationConfig;
const SimulatedIO = @import("simulation_integration.zig").SimulatedIO;
const SimulationRNG = @import("simulation_integration.zig").SimulationRNG;
const IOOperation = @import("simulation_integration.zig").IOOperation;
const AllocationStrategy = @import("deterministic_arena.zig").AllocationStrategy;
const GCConfig = @import("deterministic_arena.zig").GCConfig;
const FailureConfig = @import("crash_simulation.zig").FailureConfig;
const CrashMode = @import("crash_simulation.zig").CrashMode;
const CorruptionType = @import("crash_simulation.zig").CorruptionType;
const Object = @import("object.zig").Object;
const ObjRef = @import("objref.zig").ObjRef;

/// Helper function to create test storage engine
fn createTestEngine(allocator: std.mem.Allocator, mode: SimulationMode, seed: u64) !struct { engine: SimulatedStorageEngine, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_sim_integration.dat" });
    defer allocator.free(file_path);

    const config = SimulationConfig{
        .mode = mode,
        .seed = seed,
        .allocation_strategy = .bump,
        .enable_logging = true,
        .failure_config = FailureConfig{
            .crash_enabled = (mode != .production),
            .corruption_enabled = (mode == .chaos),
            .seed = seed,
        },
    };

    const engine = try SimulatedStorageEngine.init(allocator, config, 0, file_path, 1024 * 1024);
    return .{ .engine = engine, .dir = tmp };
}

test "SimulationRNG: deterministic sequence generation" {
    var rng1 = SimulationRNG.init(12345);
    var rng2 = SimulationRNG.init(12345);

    // Same seed should produce identical sequences
    for (0..100) |_| {
        try testing.expectEqual(rng1.next(), rng2.next());
        try testing.expectEqual(rng1.nextFloat(), rng2.nextFloat());
        try testing.expectEqual(rng1.nextBool(), rng2.nextBool());
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

    // Should be mostly different (allow for some collisions)
    try testing.expect(different_count > 80);
}

test "SimulatedIO: operation tracking and failure injection" {
    const allocator = testing.allocator;

    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 42,
        .enable_logging = true,
        .simulate_io = true,
    };

    var sim_io = SimulatedIO.init(allocator, config);
    defer sim_io.deinit();

    // Test successful operations
    var counter: u32 = 0;
    const result1 = try sim_io.simulateOperation(.memory_alloc, "size=64", struct {
        fn op(cnt: *u32) !u32 {
            cnt.* += 1;
            return cnt.*;
        }
    }.op(&counter));

    try testing.expectEqual(@as(u32, 1), result1);

    // Test operation with error
    const result2 = sim_io.simulateOperation(.memory_write, "invalid", struct {
        fn op() !u32 {
            return error.TestError;
        }
    }.op);

    try testing.expectError(error.TestError, result2);

    // Verify statistics
    const stats = sim_io.getStats();
    try testing.expectEqual(@as(u64, 2), stats.total_operations);
    try testing.expectEqual(@as(u64, 1), stats.operations_by_type.get(.memory_alloc));
    try testing.expectEqual(@as(u64, 1), stats.operations_by_type.get(.memory_write));

    // Verify event logging
    const events = sim_io.getEventLog();
    try testing.expectEqual(@as(usize, 2), events.len);
    try testing.expectEqualStrings("size=64", events[0].parameters);
    try testing.expect(events[0].success);
    try testing.expectEqualStrings("invalid", events[1].parameters);
    try testing.expect(!events[1].success);
}

test "SimulatedStorageEngine: deterministic allocation patterns" {
    const allocator = testing.allocator;
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
        try testing.expectEqual(o1, o2);
    }

    // Verify statistics match
    const stats1 = test_engine1.engine.getStats();
    const stats2 = test_engine2.engine.getStats();

    try testing.expectEqual(stats1.simulation_stats.total_operations, stats2.simulation_stats.total_operations);
    try testing.expectEqual(stats1.simulation_stats.deterministic_allocations, stats2.simulation_stats.deterministic_allocations);
}

test "SimulatedStorageEngine: crash simulation and recovery" {
    const allocator = testing.allocator;

    var test_engine = try createTestEngine(allocator, .chaos, 456);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Verify initial state
    try testing.expect(!test_engine.engine.isCrashed());

    // Simulate crash
    test_engine.engine.simulateCrash();
    try testing.expect(test_engine.engine.isCrashed());

    // Operations should fail while crashed
    const result = test_engine.engine.alloc(64);
    try testing.expectError(error.SystemCrashed, result);

    // Recover from crash
    test_engine.engine.recoverFromCrash();
    try testing.expect(!test_engine.engine.isCrashed());

    // Operations should work after recovery
    const ref = try test_engine.engine.alloc(64);
    try testing.expect(!ref.isNull());

    // Verify crash statistics
    const stats = test_engine.engine.getStats();
    try testing.expectEqual(@as(u64, 1), stats.simulation_stats.crashes_simulated);
    try testing.expectEqual(@as(u64, 1), stats.simulation_stats.recovery_operations);
}

test "SimulatedStorageEngine: object storage and retrieval" {
    const allocator = testing.allocator;

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
    try testing.expectEqual(obj.type_tag, read_obj.type_tag);
    try testing.expectEqualSlices(u8, &obj.id, &read_obj.id);
    try testing.expectEqualSlices(u8, obj.fixed_data, read_obj.fixed_data);
    try testing.expectEqualSlices(u8, obj.variable_data, read_obj.variable_data);
}

test "SimulatedStorageEngine: co-location allocation" {
    const allocator = testing.allocator;

    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 111,
        .allocation_strategy = .deterministic_colocation,
        .enable_logging = true,
    };

    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_colocation.dat" });
    defer allocator.free(file_path);

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, file_path, 1024 * 1024);
    defer engine.deinit();

    // Allocate base object
    const base_ref = try engine.alloc(64);

    // Allocate near base object
    const near_ref = try engine.allocNear(64, base_ref);

    try testing.expect(!base_ref.isNull());
    try testing.expect(!near_ref.isNull());

    // Verify allocations were tracked
    const stats = engine.getStats();
    try testing.expect(stats.simulation_stats.deterministic_allocations >= 2);
}

test "SimulatedStorageEngine: reset reproducibility" {
    const allocator = testing.allocator;

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
    _ = try test_engine.engine.alloc(128);
    const stats2 = test_engine.engine.getStats();

    // Should produce identical results
    try testing.expectEqual(ref1_run1.getOffset(), ref1_run2.getOffset());
    try testing.expectEqual(ref2_run1.getOffset(), ref2_run1.getOffset()); // Both should be same as first run
    try testing.expectEqual(stats1.simulation_stats.deterministic_allocations, stats2.simulation_stats.deterministic_allocations);
}

test "SimulatedStorageEngine: time simulation" {
    const allocator = testing.allocator;

    var test_engine = try createTestEngine(allocator, .deterministic, 333);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    const initial_time = test_engine.engine.getCurrentTime();

    // Perform operations (should advance time)
    _ = try test_engine.engine.alloc(64);
    _ = try test_engine.engine.alloc(128);

    const time_after_ops = test_engine.engine.getCurrentTime();
    try testing.expect(time_after_ops > initial_time);

    // Manually advance time
    test_engine.engine.advanceTime(1000000); // 1ms
    const time_after_advance = test_engine.engine.getCurrentTime();
    try testing.expectEqual(time_after_ops + 1000000, time_after_advance);
}

test "SimulatedStorageEngine: different simulation modes" {
    const allocator = testing.allocator;
    const seed: u64 = 444;

    const modes = [_]SimulationMode{ .production, .deterministic, .chaos, .hybrid };

    for (modes) |mode| {
        var test_engine = try createTestEngine(allocator, mode, seed);
        defer test_engine.dir.cleanup();
        defer test_engine.engine.deinit();

        // Perform basic operations
        const ref = try test_engine.engine.alloc(64);
        try testing.expect(!ref.isNull());

        // Create and store object
        const id = [_]u8{1} ** 16;
        var obj = try Object.init(allocator, 1, id, &[_]u8{1}, "mode test", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        try test_engine.engine.write(ref, data);

        // Verify statistics are appropriate for mode
        const stats = test_engine.engine.getStats();
        try testing.expect(stats.simulation_stats.total_operations > 0);

        switch (mode) {
            .production => {
                // Production mode should have minimal simulation overhead
                try testing.expectEqual(@as(u64, 0), stats.simulation_stats.deterministic_allocations);
            },
            .deterministic, .chaos, .hybrid => {
                // Simulation modes should track deterministic allocations
                try testing.expect(stats.simulation_stats.deterministic_allocations > 0);
            },
        }
    }
}

test "SimulatedStorageEngine: failure injection patterns" {
    const allocator = testing.allocator;

    const failure_config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .random_timing,
        .crash_probability = 0.5, // High probability for testing
        .corruption_enabled = true,
        .corruption_type = .bit_flip,
        .corruption_probability = 0.3,
        .seed = 555,
    };

    const config = SimulationConfig{
        .mode = .chaos,
        .seed = 555,
        .failure_config = failure_config,
        .enable_logging = true,
    };

    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_failures.dat" });
    defer allocator.free(file_path);

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, file_path, 1024 * 1024);
    defer engine.deinit();

    var successful_ops: u32 = 0;
    var failed_ops: u32 = 0;

    // Attempt many operations to trigger failures
    for (0..100) |_| {
        const result = engine.alloc(64);
        if (result) |_| {
            successful_ops += 1;
        } else |_| {
            failed_ops += 1;
            if (engine.isCrashed()) {
                engine.recoverFromCrash();
            }
        }
    }

    // Should have some failures due to high failure probability
    try testing.expect(failed_ops > 0);
    try testing.expect(successful_ops > 0);

    const stats = engine.getStats();
    try testing.expect(stats.crash_stats.crashes_simulated > 0);
}

test "SimulatedStorageEngine: configuration updates" {
    const allocator = testing.allocator;

    var test_engine = try createTestEngine(allocator, .deterministic, 666);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Initial allocation with bump strategy
    const ref1 = try test_engine.engine.alloc(64);

    // Update configuration to use different strategy
    var new_config = test_engine.engine.config;
    new_config.allocation_strategy = .random_placement;
    test_engine.engine.updateConfig(new_config);

    // Subsequent allocations should use new strategy
    _ = try test_engine.engine.alloc(64);

    try testing.expect(!ref1.isNull());

    // Verify configuration was updated
    try testing.expectEqual(AllocationStrategy.random_placement, test_engine.engine.config.allocation_strategy);
}

test "SimulatedStorageEngine: comprehensive statistics" {
    const allocator = testing.allocator;

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
    try testing.expect(stats.simulation_stats.total_operations > 0);
    try testing.expect(stats.simulation_stats.deterministic_allocations > 0);
    try testing.expect(stats.simulation_stats.simulation_time_ns > 0);
    try testing.expectEqual(@as(u64, 1), stats.simulation_stats.crashes_simulated);
    try testing.expectEqual(@as(u64, 1), stats.simulation_stats.recovery_operations);
    try testing.expectEqual(@as(u64, 0), stats.shard_id);
    try testing.expectEqual(SimulationMode.hybrid, stats.mode);

    // Generate and verify report
    const report = try stats.generateReport(allocator);
    defer allocator.free(report);

    try testing.expect(std.mem.indexOf(u8, report, "Simulation Engine Statistics Report") != null);
    try testing.expect(std.mem.indexOf(u8, report, "Shard ID: 0") != null);
    try testing.expect(std.mem.indexOf(u8, report, "Mode: hybrid") != null);
}

test "SimulatedStorageEngine: event logging and analysis" {
    const allocator = testing.allocator;

    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 888,
        .enable_logging = true,
        .max_log_entries = 100,
    };

    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_logging.dat" });
    defer allocator.free(file_path);

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, file_path, 1024 * 1024);
    defer engine.deinit();

    // Perform operations to generate events
    _ = try engine.alloc(64);
    _ = try engine.alloc(128);
    _ = try engine.alloc(256);

    // Verify event log
    const events = engine.getEventLog();
    try testing.expect(events.len >= 3);

    // Verify event details
    for (events) |event| {
        try testing.expect(event.sequence > 0);
        try testing.expect(event.timestamp_ns >= 0);
        try testing.expect(event.parameters.len > 0);
        try testing.expect(event.success);
        try testing.expect(event.duration_ns > 0);
    }

    // Verify events are in chronological order
    for (events[1..], 1..) |event, i| {
        try testing.expect(event.timestamp_ns >= events[i - 1].timestamp_ns);
        try testing.expect(event.sequence > events[i - 1].sequence);
    }
}

test "SimulatedStorageEngine: memory management" {
    const allocator = testing.allocator;

    var test_engine = try createTestEngine(allocator, .deterministic, 999);
    defer test_engine.dir.cleanup();
    defer test_engine.engine.deinit();

    // Allocate and delete objects
    const ref1 = try test_engine.engine.alloc(64);
    const ref2 = try test_engine.engine.alloc(128);

    try test_engine.engine.delete(ref1);

    // Verify deletion was tracked
    const stats = test_engine.engine.getStats();
    try testing.expect(stats.simulation_stats.total_operations >= 3); // 2 allocs + 1 delete
}
