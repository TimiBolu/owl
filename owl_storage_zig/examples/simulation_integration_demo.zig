//! Demonstration of Zig Simulation Integration
//!
//! This example shows how the SimulatedStorageEngine provides comprehensive
//! simulation capabilities for testing distributed storage scenarios.

const std = @import("std");
const owl_storage = @import("owl_storage");
const SimulatedStorageEngine = owl_storage.SimulatedStorageEngine;
const SimulationMode = owl_storage.SimulationMode;
const SimulationConfig = owl_storage.SimulationConfig;
const AllocationStrategy = owl_storage.AllocationStrategy;
const GCConfig = owl_storage.GCConfig;
const FailureConfig = owl_storage.FailureConfig;
const CrashMode = owl_storage.CrashMode;
const CorruptionType = owl_storage.CorruptionType;
const Object = owl_storage.Object;
const ObjRef = owl_storage.ObjRef;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("=== Zig Simulation Integration Demo ===\n\n");

    // Create temporary directory for demo
    var tmp_dir = std.testing.tmpDir(.{});
    defer tmp_dir.cleanup();

    const path = try tmp_dir.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const storage_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "demo_storage.dat" });
    defer allocator.free(storage_path);

    // Demo 1: Deterministic simulation mode
    try demoDeterministicMode(allocator, storage_path);

    // Demo 2: Crash simulation and recovery
    try demoCrashSimulation(allocator, storage_path);

    // Demo 3: Chaos mode with failure injection
    try demoChaosMode(allocator, storage_path);

    // Demo 4: Performance comparison across modes
    try demoPerformanceComparison(allocator, storage_path);

    // Demo 5: Reproducible testing scenarios
    try demoReproducibleTesting(allocator, storage_path);

    std.debug.print("=== Demo Complete ===\n");
}

fn demoDeterministicMode(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("1. Deterministic Simulation Mode\n");
    std.debug.print("   Testing reproducible behavior with seed-based determinism...\n");

    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 42,
        .allocation_strategy = .bump,
        .enable_logging = true,
        .simulate_io = true,
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    // Perform a series of operations
    std.debug.print("   Performing allocation sequence...\n");
    const sizes = [_]u32{ 64, 128, 256, 512, 1024 };
    var refs: [5]ObjRef = undefined;

    for (sizes, 0..) |size, i| {
        refs[i] = try engine.alloc(size);
        std.debug.print("     Allocation {}: size={d}, offset={d}\n", .{ i + 1, size, refs[i].getOffset() });
    }

    // Create and store some objects
    for (refs, 0..) |ref, i| {
        const id = [_]u8{@intCast(i + 1)} ** 16;
        var obj = try Object.init(allocator, @intCast(i + 1), id, &[_]u8{ @intCast(i), @intCast(i + 1) }, "test data", &[_]ObjRef{});
        defer obj.deinit();

        const data = try obj.toBytes(allocator);
        defer allocator.free(data);

        try engine.write(ref, data);
        std.debug.print("     Stored object {}: type={d}, id={d}\n", .{ i + 1, obj.type_tag, obj.id[0] });
    }

    // Get statistics
    const stats = engine.getStats();
    std.debug.print("   Results:\n");
    std.debug.print("     Total operations: {d}\n", .{stats.simulation_stats.total_operations});
    std.debug.print("     Deterministic allocations: {d}\n", .{stats.simulation_stats.deterministic_allocations});
    std.debug.print("     Simulation time: {d} ns\n", .{stats.simulation_stats.simulation_time_ns});
    std.debug.print("     Speedup factor: {d:.2}x\n\n", .{stats.simulation_stats.speedupFactor()});
}

fn demoCrashSimulation(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("2. Crash Simulation and Recovery\n");
    std.debug.print("   Testing crash scenarios and recovery mechanisms...\n");

    const failure_config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .random_timing,
        .crash_probability = 0.3, // 30% chance of crash
        .corruption_enabled = true,
        .corruption_type = .bit_flip,
        .corruption_probability = 0.1, // 10% chance of corruption
        .seed = 123,
    };

    const config = SimulationConfig{
        .mode = .hybrid,
        .seed = 123,
        .allocation_strategy = .random_placement,
        .failure_config = failure_config,
        .enable_logging = true,
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    std.debug.print("   Performing operations with crash simulation...\n");

    var successful_ops: u32 = 0;
    var failed_ops: u32 = 0;

    // Attempt multiple operations, some may fail due to crashes
    for (0..20) |i| {
        const size: u32 = 64 + @as(u32, @intCast(i)) * 32;

        const result = engine.alloc(size);
        if (result) |ref| {
            std.debug.print("     Operation {}: SUCCESS - allocated at offset {d}\n", .{ i + 1, ref.getOffset() });
            successful_ops += 1;

            // Try to write data
            const id = [_]u8{@intCast(i + 1)} ** 16;
            var obj = Object.init(allocator, @intCast(i + 1), id, &[_]u8{@intCast(i)}, "crash test", &[_]ObjRef{}) catch continue;
            defer obj.deinit();

            const data = obj.toBytes(allocator) catch continue;
            defer allocator.free(data);

            engine.write(ref, data) catch |err| {
                std.debug.print("     Write failed: {s}\n", .{@errorName(err)});
                if (engine.isCrashed()) {
                    std.debug.print("     System crashed! Recovering...\n");
                    engine.recoverFromCrash();
                }
            };
        } else |err| {
            std.debug.print("     Operation {}: FAILED - {s}\n", .{ i + 1, @errorName(err) });
            failed_ops += 1;

            if (engine.isCrashed()) {
                std.debug.print("     System crashed! Recovering...\n");
                engine.recoverFromCrash();
            }
        }
    }

    const stats = engine.getStats();
    std.debug.print("   Results:\n");
    std.debug.print("     Successful operations: {d}\n", .{successful_ops});
    std.debug.print("     Failed operations: {d}\n", .{failed_ops});
    std.debug.print("     Crashes simulated: {d}\n", .{stats.crash_stats.crashes_simulated});
    std.debug.print("     Corruptions injected: {d}\n", .{stats.crash_stats.corruptions_injected});
    std.debug.print("     Recovery operations: {d}\n", .{stats.simulation_stats.recovery_operations});
    std.debug.print("     Partial writes: {d}\n\n", .{stats.crash_stats.partial_writes});
}

fn demoChaosMode(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("3. Chaos Mode with Failure Injection\n");
    std.debug.print("   Testing system resilience under extreme conditions...\n");

    const failure_config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .random_timing,
        .crash_probability = 0.2,
        .corruption_enabled = true,
        .corruption_type = .header_corruption,
        .corruption_probability = 0.15,
        .max_corruption_bytes = 8,
        .seed = 456,
    };

    const config = SimulationConfig{
        .mode = .chaos,
        .seed = 456,
        .allocation_strategy = .fragmentation_sim,
        .failure_config = failure_config,
        .enable_logging = true,
        .time_acceleration = 2000.0, // 2000x speedup
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    std.debug.print("   Running chaos test with high failure rates...\n");

    var operations_attempted: u32 = 0;
    var operations_succeeded: u32 = 0;
    var corruption_detected: u32 = 0;

    // Stress test with many operations
    for (0..50) |i| {
        operations_attempted += 1;

        // Try allocation
        const alloc_result = engine.alloc(128);
        if (alloc_result) |ref| {
            // Try to create and write object
            const id = [_]u8{@intCast(i % 256)} ** 16;
            if (Object.init(allocator, @intCast(i % 256), id, &[_]u8{@intCast(i % 256)}, "chaos test", &[_]ObjRef{})) |obj| {
                var obj_mut = obj;
                defer obj_mut.deinit();

                if (obj.toBytes(allocator)) |data| {
                    defer allocator.free(data);

                    if (engine.write(ref, data)) |_| {
                        // Try to read back
                        if (engine.read(ref, allocator)) |read_obj| {
                            var read_obj_mut = read_obj;
                            defer read_obj_mut.deinit();
                            operations_succeeded += 1;
                        } else |read_err| {
                            if (read_err == error.ChecksumMismatch or read_err == error.InsufficientData) {
                                corruption_detected += 1;
                            }
                        }
                    } else |write_err| {
                        if (engine.isCrashed()) {
                            engine.recoverFromCrash();
                        }
                    }
                } else |_| {}
            } else |_| {}
        } else |alloc_err| {
            if (engine.isCrashed()) {
                engine.recoverFromCrash();
            }
        }

        // Occasionally simulate manual crashes for testing
        if (i % 10 == 0) {
            engine.simulateCrash();
            engine.recoverFromCrash();
        }
    }

    const stats = engine.getStats();
    std.debug.print("   Chaos test results:\n");
    std.debug.print("     Operations attempted: {d}\n", .{operations_attempted});
    std.debug.print("     Operations succeeded: {d}\n", .{operations_succeeded});
    std.debug.print("     Success rate: {d:.1}%\n", .{@as(f64, @floatFromInt(operations_succeeded)) / @as(f64, @floatFromInt(operations_attempted)) * 100.0});
    std.debug.print("     Corruption detected: {d}\n", .{corruption_detected});
    std.debug.print("     Total crashes: {d}\n", .{stats.crash_stats.crashes_simulated});
    std.debug.print("     Total corruptions: {d}\n", .{stats.crash_stats.corruptions_injected});
    std.debug.print("     Simulation speedup: {d:.1}x\n\n", .{stats.simulation_stats.speedupFactor()});
}

fn demoPerformanceComparison(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("4. Performance Comparison Across Modes\n");
    std.debug.print("   Comparing performance overhead of different simulation modes...\n");

    const modes = [_]SimulationMode{ .production, .deterministic, .chaos };
    const mode_names = [_][]const u8{ "Production", "Deterministic", "Chaos" };

    for (modes, mode_names) |mode, name| {
        std.debug.print("   Testing {s} mode:\n", .{name});

        const config = SimulationConfig{
            .mode = mode,
            .seed = 789,
            .allocation_strategy = .bump,
            .failure_config = FailureConfig{
                .crash_enabled = (mode != .production),
                .crash_probability = if (mode == .chaos) 0.05 else 0.01,
                .corruption_enabled = (mode == .chaos),
                .seed = 789,
            },
            .enable_logging = false, // Disable for performance
        };

        var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
        defer engine.deinit();

        const start_time = std.time.nanoTimestamp();

        // Perform benchmark operations
        var successful_ops: u32 = 0;
        for (0..1000) |i| {
            const size: u32 = 64 + @as(u32, @intCast(i % 10)) * 16;

            if (engine.alloc(size)) |ref| {
                const id = [_]u8{@intCast(i % 256)} ** 16;
                if (Object.init(allocator, @intCast(i % 256), id, &[_]u8{@intCast(i % 256)}, "perf test", &[_]ObjRef{})) |obj| {
                    var obj_mut = obj;
                    defer obj_mut.deinit();

                    if (obj.toBytes(allocator)) |data| {
                        defer allocator.free(data);

                        if (engine.write(ref, data)) |_| {
                            successful_ops += 1;
                        } else |_| {
                            if (engine.isCrashed()) {
                                engine.recoverFromCrash();
                            }
                        }
                    } else |_| {}
                } else |_| {}
            } else |_| {
                if (engine.isCrashed()) {
                    engine.recoverFromCrash();
                }
            }
        }

        const end_time = std.time.nanoTimestamp();
        const duration_ns = @as(u64, @intCast(end_time - start_time));
        const ops_per_sec = @as(f64, @floatFromInt(successful_ops)) / (@as(f64, @floatFromInt(duration_ns)) / 1_000_000_000.0);

        const stats = engine.getStats();
        std.debug.print("     Successful operations: {d}/1000\n", .{successful_ops});
        std.debug.print("     Duration: {d} ms\n", .{duration_ns / 1_000_000});
        std.debug.print("     Operations/sec: {d:.0}\n", .{ops_per_sec});
        std.debug.print("     Simulation speedup: {d:.1}x\n", .{stats.simulation_stats.speedupFactor()});
        std.debug.print("     Crashes: {d}, Corruptions: {d}\n", .{ stats.crash_stats.crashes_simulated, stats.crash_stats.corruptions_injected });
    }
    std.debug.print("\n");
}

fn demoReproducibleTesting(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    std.debug.print("5. Reproducible Testing Scenarios\n");
    std.debug.print("   Demonstrating identical results with same seed...\n");

    const seed: u64 = 999;
    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = seed,
        .allocation_strategy = .random_placement,
        .failure_config = FailureConfig{
            .crash_enabled = true,
            .crash_probability = 0.1,
            .seed = seed,
        },
        .enable_logging = true,
    };

    // Run 1
    std.debug.print("   Run 1 with seed {d}:\n", .{seed});
    var engine1 = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine1.deinit();

    var results1: [5]u64 = undefined;
    for (0..5) |i| {
        if (engine1.alloc(128)) |ref| {
            results1[i] = ref.getOffset();
            std.debug.print("     Allocation {}: offset={d}\n", .{ i + 1, ref.getOffset() });
        } else |_| {
            results1[i] = 0;
            std.debug.print("     Allocation {}: FAILED\n", .{i + 1});
            if (engine1.isCrashed()) {
                engine1.recoverFromCrash();
            }
        }
    }

    const stats1 = engine1.getStats();

    // Run 2 with same seed
    std.debug.print("   Run 2 with same seed {d}:\n", .{seed});
    var engine2 = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine2.deinit();

    var results2: [5]u64 = undefined;
    for (0..5) |i| {
        if (engine2.alloc(128)) |ref| {
            results2[i] = ref.getOffset();
            std.debug.print("     Allocation {}: offset={d}\n", .{ i + 1, ref.getOffset() });
        } else |_| {
            results2[i] = 0;
            std.debug.print("     Allocation {}: FAILED\n", .{i + 1});
            if (engine2.isCrashed()) {
                engine2.recoverFromCrash();
            }
        }
    }

    const stats2 = engine2.getStats();

    // Verify reproducibility
    var matches: u32 = 0;
    for (results1, results2) |r1, r2| {
        if (r1 == r2) matches += 1;
    }

    std.debug.print("   Reproducibility results:\n");
    std.debug.print("     Matching allocations: {d}/5\n", .{matches});
    std.debug.print("     Run 1 crashes: {d}, Run 2 crashes: {d}\n", .{ stats1.crash_stats.crashes_simulated, stats2.crash_stats.crashes_simulated });
    std.debug.print("     Run 1 operations: {d}, Run 2 operations: {d}\n", .{ stats1.simulation_stats.total_operations, stats2.simulation_stats.total_operations });
    std.debug.print("     Deterministic behavior: {}\n\n", .{matches == 5 and stats1.crash_stats.crashes_simulated == stats2.crash_stats.crashes_simulated});
}
