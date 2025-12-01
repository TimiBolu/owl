//! Simple demonstration of Zig Simulation Integration
//!
//! This example shows the core simulation capabilities implemented for task 8.11.

const std = @import("std");
const SimulatedStorageEngine = @import("../src/simulation_integration_simple.zig").SimulatedStorageEngine;
const SimulationMode = @import("../src/simulation_integration_simple.zig").SimulationMode;
const SimulationConfig = @import("../src/simulation_integration_simple.zig").SimulationConfig;
const AllocationStrategy = @import("../src/deterministic_arena.zig").AllocationStrategy;
const GCConfig = @import("../src/deterministic_arena.zig").GCConfig;
const FailureConfig = @import("../src/crash_simulation.zig").FailureConfig;
const CrashMode = @import("../src/crash_simulation.zig").CrashMode;
const Object = @import("../src/object.zig").Object;
const ObjRef = @import("../src/objref.zig").ObjRef;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("=== Zig Simulation Integration Demo ===\n\n", .{});

    // Create temporary directory for demo
    var tmp_dir = std.testing.tmpDir(.{});
    defer tmp_dir.cleanup();

    const path = try tmp_dir.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const storage_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "demo_storage.dat" });
    defer allocator.free(storage_path);

    // Demo 1: Deterministic mode
    try demoDeterministicMode(allocator, storage_path);

    // Demo 2: Crash simulation
    try demoCrashSimulation(allocator, storage_path);

    // Demo 3: Reproducible testing
    try demoReproducibleTesting(allocator, storage_path);

    std.debug.print("=== Demo Complete ===\n", .{});
}

fn demoDeterministicMode(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("1. Deterministic Simulation Mode\n", .{});

    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 42,
        .allocation_strategy = .bump,
        .enable_logging = true,
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    // Perform allocations
    const sizes = [_]u32{ 64, 128, 256, 512 };
    std.debug.print("   Performing deterministic allocations:\n", .{});

    for (sizes, 0..) |size, i| {
        const ref = try engine.alloc(size);
        std.debug.print("     Allocation {}: size={d}, offset={d}\n", .{ i + 1, size, ref.getOffset() });
    }

    // Create and store objects
    for (0..3) |i| {
        const id = [_]u8{@intCast(i + 1)} ** 16;
        var obj = try Object.init(allocator, @intCast(i + 1), id, &[_]u8{@intCast(i)}, "test data", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        const ref = try engine.alloc(@intCast(data.len));
        try engine.write(ref, data);

        std.debug.print("     Stored object {}: type={d}, size={d}\n", .{ i + 1, obj.header.type_tag, data.len });
    }

    const stats = engine.getStats();
    std.debug.print("   Results: operations={d}, deterministic_allocations={d}\n\n", .{ stats.simulation_stats.total_operations, stats.simulation_stats.deterministic_allocations });
}

fn demoCrashSimulation(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("2. Crash Simulation and Recovery\n", .{});

    const config = SimulationConfig{
        .mode = .chaos,
        .seed = 123,
        .failure_config = FailureConfig{
            .crash_enabled = true,
            .crash_mode = .random_timing,
            .crash_probability = 0.3,
            .seed = 123,
        },
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    std.debug.print("   Testing crash scenarios:\n", .{});

    // Manual crash simulation
    std.debug.print("     Simulating crash...\n", .{});
    engine.simulateCrash();
    std.debug.print("     System crashed: {}\n", .{engine.isCrashed()});

    // Try operation while crashed
    const result = engine.alloc(64);
    if (result) |ref| {
        std.debug.print("     Unexpected success: {d}\n", .{ref.getOffset()});
    } else |err| {
        std.debug.print("     Expected failure: {s}\n", .{@errorName(err)});
    }

    // Recover and try again
    std.debug.print("     Recovering from crash...\n", .{});
    engine.recoverFromCrash();
    std.debug.print("     System recovered: {}\n", .{!engine.isCrashed()});

    const ref = try engine.alloc(64);
    std.debug.print("     Successful allocation after recovery: offset={d}\n", .{ref.getOffset()});

    const stats = engine.getStats();
    std.debug.print("   Results: crashes={d}, recoveries={d}\n\n", .{ stats.simulation_stats.crashes_simulated, stats.simulation_stats.recovery_operations });
}

fn demoReproducibleTesting(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("3. Reproducible Testing\n", .{});

    const seed: u64 = 999;
    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = seed,
        .allocation_strategy = .random_placement,
    };

    std.debug.print("   Run 1 with seed {d}:\n", .{seed});
    var engine1 = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine1.deinit();

    var results1: [3]u64 = undefined;
    for (0..3) |i| {
        const ref = try engine1.alloc(128);
        results1[i] = ref.getOffset();
        std.debug.print("     Allocation {}: offset={d}\n", .{ i + 1, ref.getOffset() });
    }

    std.debug.print("   Run 2 with same seed {d}:\n", .{seed});
    var engine2 = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine2.deinit();

    var results2: [3]u64 = undefined;
    for (0..3) |i| {
        const ref = try engine2.alloc(128);
        results2[i] = ref.getOffset();
        std.debug.print("     Allocation {}: offset={d}\n", .{ i + 1, ref.getOffset() });
    }

    // Check reproducibility
    var matches: u32 = 0;
    for (results1, results2) |r1, r2| {
        if (r1 == r2) matches += 1;
    }

    std.debug.print("   Reproducibility: {d}/3 allocations match\n", .{matches});
    std.debug.print("   Deterministic behavior: {}\n\n", .{matches == 3});
}
