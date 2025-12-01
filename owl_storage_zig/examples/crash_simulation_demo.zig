//! Crash Simulation Demo
//!
//! This example demonstrates the crash simulation capabilities of the Zig storage layer.
//! It shows how to:
//! 1. Configure different types of failures and corruption
//! 2. Test crash recovery scenarios
//! 3. Verify data integrity after crashes
//! 4. Use deterministic testing for reproducible results

const std = @import("std");
const print = std.debug.print;
const owl_storage = @import("storage");

const Arena = owl_storage.Arena;
const Object = owl_storage.Object;
const ObjectHeader = owl_storage.ObjectHeader;
const ObjRef = owl_storage.ObjRef;
const WAL = owl_storage.WAL;
const CrashSimulator = owl_storage.CrashSimulator;
const CrashAwareArena = owl_storage.CrashAwareArena;
const CrashAwareWAL = owl_storage.CrashAwareWAL;
const FailureConfig = owl_storage.FailureConfig;
const CrashMode = owl_storage.CrashMode;
const CorruptionType = owl_storage.CorruptionType;

/// Demo configuration for different crash scenarios
const DemoConfig = struct {
    name: []const u8,
    config: FailureConfig,
    description: []const u8,
};

const demo_configs = [_]DemoConfig{
    .{
        .name = "crash_before_write",
        .config = FailureConfig{
            .crash_enabled = true,
            .crash_mode = .before_write,
            .crash_probability = 0.3,
            .seed = 12345,
        },
        .description = "Crashes before write operations (30% probability)",
    },
    .{
        .name = "crash_during_write",
        .config = FailureConfig{
            .crash_enabled = true,
            .crash_mode = .during_write,
            .crash_probability = 0.5,
            .seed = 23456,
        },
        .description = "Crashes during write operations causing partial writes (50% probability)",
    },
    .{
        .name = "bit_flip_corruption",
        .config = FailureConfig{
            .corruption_enabled = true,
            .corruption_type = .bit_flip,
            .corruption_probability = 0.4,
            .max_corruption_bytes = 8,
            .seed = 34567,
        },
        .description = "Injects bit-flip corruption (40% probability, up to 8 bytes)",
    },
    .{
        .name = "header_corruption",
        .config = FailureConfig{
            .corruption_enabled = true,
            .corruption_type = .header_corruption,
            .corruption_probability = 0.6,
            .seed = 45678,
        },
        .description = "Corrupts object headers (60% probability)",
    },
    .{
        .name = "wal_corruption",
        .config = FailureConfig{
            .corruption_enabled = true,
            .corruption_type = .wal_corruption,
            .corruption_probability = 0.3,
            .seed = 56789,
        },
        .description = "Corrupts WAL entries (30% probability)",
    },
    .{
        .name = "combined_failures",
        .config = FailureConfig{
            .crash_enabled = true,
            .crash_mode = .random_timing,
            .crash_probability = 0.2,
            .corruption_enabled = true,
            .corruption_type = .bit_flip,
            .corruption_probability = 0.15,
            .max_corruption_bytes = 4,
            .seed = 67890,
        },
        .description = "Combined crashes and corruption (20% crash, 15% corruption)",
    },
};

/// Create a test arena
fn createArena(allocator: std.mem.Allocator, name: []const u8) !Arena {
    const path = try std.fmt.allocPrint(allocator, "/tmp/crash_demo_{s}.dat", .{name});
    defer allocator.free(path);

    // Remove existing file
    std.fs.cwd().deleteFile(path) catch {};

    return Arena.init(allocator, path, 1024 * 1024, 0); // 1MB, shard_id = 0
}

/// Create a test WAL
fn createWAL(allocator: std.mem.Allocator, name: []const u8) !WAL {
    const path = try std.fmt.allocPrint(allocator, "/tmp/crash_demo_wal_{s}", .{name});
    defer allocator.free(path);

    // Remove existing directory
    std.fs.cwd().deleteTree(path) catch {};

    return WAL.init(allocator, path);
}

/// Demo arena operations with crash simulation
fn demoArenaOperations(allocator: std.mem.Allocator, config: DemoConfig) !void {
    print("\n=== Arena Demo: {s} ===\n", .{config.name});
    print("Description: {s}\n", .{config.description});

    var simulator = CrashSimulator.init(allocator, config.config);
    defer simulator.deinit();

    var arena = try createArena(allocator, config.name);
    defer arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &arena, &simulator);

    var successful_ops: u32 = 0;
    var failed_ops: u32 = 0;
    var corruption_detected: u32 = 0;

    // Perform multiple operations
    for (0..20) |i| {
        // Create test object
        const id = [_]u8{@intCast(i % 256)} ** 16;
        var obj = Object.init(
            allocator,
            @intCast(i % 5),
            id,
            &[_]u8{ @intCast(i), @intCast(i + 1), @intCast(i + 2) },
            "test data for crash simulation",
            &[_]ObjRef{},
        ) catch continue;
        defer obj.deinit();

        const data = obj.toBytes(allocator) catch continue;
        defer allocator.free(data);

        // Try to allocate and write
        const alloc_result = crash_arena.alloc(@intCast(data.len));
        if (alloc_result) |objref| {
            const write_result = crash_arena.write(objref, data);
            if (write_result) |_| {
                // Try to read back
                const read_result = crash_arena.read(objref, allocator);
                if (read_result) |read_obj| {
                    var read_obj_mut = read_obj;
                    defer read_obj_mut.deinit();

                    // Verify integrity
                    const verify_result = read_obj_mut.verify();
                    if (verify_result) |_| {
                        successful_ops += 1;
                    } else |_| {
                        corruption_detected += 1;
                    }
                } else |_| {
                    corruption_detected += 1;
                }
            } else |_| {
                failed_ops += 1;
                simulator.recover();
            }
        } else |_| {
            failed_ops += 1;
            simulator.recover();
        }
    }

    // Print results
    const stats = simulator.getStats();
    print("Results:\n", .{});
    print("  Successful operations: {}\n", .{successful_ops});
    print("  Failed operations: {}\n", .{failed_ops});
    print("  Corruption detected: {}\n", .{corruption_detected});
    print("  Total crashes: {}\n", .{stats.crashes_simulated});
    print("  Total corruptions: {}\n", .{stats.corruptions_injected});
    print("  Partial writes: {}\n", .{stats.partial_writes});

    if (stats.operations_attempted > 0) {
        const success_rate = (@as(f64, @floatFromInt(successful_ops)) /
            @as(f64, @floatFromInt(stats.operations_attempted))) * 100.0;
        print("  Success rate: {d:.1}%\n", .{success_rate});
    }
}

/// Demo WAL operations with crash simulation
fn demoWALOperations(allocator: std.mem.Allocator, config: DemoConfig) !void {
    print("\n=== WAL Demo: {s} ===\n", .{config.name});
    print("Description: {s}\n", .{config.description});

    var simulator = CrashSimulator.init(allocator, config.config);
    defer simulator.deinit();

    var wal = try createWAL(allocator, config.name);
    defer wal.deinit();

    var crash_wal = CrashAwareWAL.init(allocator, &wal, &simulator);

    var successful_entries: u32 = 0;
    var failed_entries: u32 = 0;

    // Write multiple WAL entries
    for (0..15) |i| {
        const data = try std.fmt.allocPrint(allocator, "WAL entry {} for crash simulation demo", .{i});
        defer allocator.free(data);

        const objref = ObjRef.init(1000 + i * 100);
        var entry = owl_storage.WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data) catch continue;
        defer entry.deinit(allocator);

        const result = crash_wal.append(&entry);
        if (result) |_| {
            successful_entries += 1;
        } else |_| {
            failed_entries += 1;
            simulator.recover();
        }
    }

    // Print results
    const stats = simulator.getStats();
    const wal_stats = wal.getStats();

    print("Results:\n", .{});
    print("  Successful WAL entries: {}\n", .{successful_entries});
    print("  Failed WAL entries: {}\n", .{failed_entries});
    print("  Total WAL entries written: {}\n", .{wal_stats.entry_count});
    print("  Total crashes: {}\n", .{stats.crashes_simulated});
    print("  Total corruptions: {}\n", .{stats.corruptions_injected});

    if (successful_entries + failed_entries > 0) {
        const success_rate = (@as(f64, @floatFromInt(successful_entries)) /
            @as(f64, @floatFromInt(successful_entries + failed_entries))) * 100.0;
        print("  Success rate: {d:.1}%\n", .{success_rate});
    }
}

/// Demo deterministic behavior
fn demoDeterministicBehavior(allocator: std.mem.Allocator) !void {
    print("\n=== Deterministic Behavior Demo ===\n", .{});
    print("Description: Same seed produces identical crash patterns\n", .{});

    const seed: u64 = 99999;
    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .random_timing,
        .crash_probability = 0.4,
        .seed = seed,
    };

    // Run simulation twice with same seed
    var results1: [50]bool = undefined;
    var results2: [50]bool = undefined;

    // First run
    {
        var simulator = CrashSimulator.init(allocator, config);
        defer simulator.deinit();

        for (&results1) |*result| {
            result.* = simulator.shouldCrash("test_op");
        }
    }

    // Second run with same seed
    {
        var simulator = CrashSimulator.init(allocator, config);
        defer simulator.deinit();

        for (&results2) |*result| {
            result.* = simulator.shouldCrash("test_op");
        }
    }

    // Compare results
    var identical_count: u32 = 0;
    for (results1, results2) |r1, r2| {
        if (r1 == r2) {
            identical_count += 1;
        }
    }

    print("Results:\n", .{});
    print("  Total operations: {}\n", .{results1.len});
    print("  Identical results: {}\n", .{identical_count});
    print("  Deterministic: {}\n", .{identical_count == results1.len});

    // Show first 10 results for verification
    print("  First 10 crash decisions (run 1): ", .{});
    for (results1[0..10]) |result| {
        print("{} ", .{@as(u8, if (result) 1 else 0)});
    }
    print("\n", .{});

    print("  First 10 crash decisions (run 2): ", .{});
    for (results2[0..10]) |result| {
        print("{} ", .{@as(u8, if (result) 1 else 0)});
    }
    print("\n", .{});
}

/// Demo recovery scenarios
fn demoRecoveryScenarios(allocator: std.mem.Allocator) !void {
    print("\n=== Recovery Scenarios Demo ===\n", .{});
    print("Description: Testing crash recovery and data consistency\n", .{});

    const config = FailureConfig{
        .crash_enabled = true,
        .crash_mode = .after_write_before_sync,
        .crash_probability = 1.0, // Always crash
        .seed = 77777,
    };

    var simulator = CrashSimulator.init(allocator, config);
    defer simulator.deinit();

    var arena = try createArena(allocator, "recovery_test");
    defer arena.deinit();

    var crash_arena = CrashAwareArena.init(allocator, &arena, &simulator);

    // Scenario 1: Write that crashes before sync
    print("\nScenario 1: Write crashes before sync\n", .{});
    const test_data = "Important data that should survive crash";
    const objref = try arena.alloc(@intCast(test_data.len));

    const result1 = crash_arena.write(objref, test_data);
    if (result1) |_| {
        print("  Unexpected: Write succeeded when it should have crashed\n", .{});
    } else |err| {
        print("  Expected: Write failed with error: {}\n", .{err});
        print("  System crashed: {}\n", .{simulator.isCrashed()});
    }

    // Scenario 2: Recovery and successful write
    print("\nScenario 2: Recovery and retry\n", .{});
    simulator.recover();
    simulator.config.crash_enabled = false; // Disable crash for recovery

    const result2 = crash_arena.write(objref, test_data);
    if (result2) |_| {
        print("  Success: Write completed after recovery\n", .{});

        // Verify data integrity
        const read_data = arena.readAt(objref.getOffset(), test_data.len) catch "READ_FAILED";
        if (std.mem.eql(u8, test_data, read_data)) {
            print("  Success: Data integrity verified\n", .{});
        } else {
            print("  Error: Data corruption detected\n", .{});
        }
    } else |err| {
        print("  Error: Write failed after recovery: {}\n", .{err});
    }

    // Print final statistics
    const stats = simulator.getStats();
    print("\nRecovery Statistics:\n", .{});
    print("  Total crashes: {}\n", .{stats.crashes_simulated});
    print("  Recovery attempts: {}\n", .{stats.recovery_attempts});
}

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    print("Owl Storage Zig - Crash Simulation Demo\n", .{});
    print("======================================\n", .{});
    print("This demo shows crash simulation capabilities for testing storage reliability.\n", .{});

    // Run arena operation demos for each configuration
    for (demo_configs) |config| {
        demoArenaOperations(allocator, config) catch |err| {
            print("Error in arena demo {s}: {}\n", .{ config.name, err });
        };
    }

    // Run WAL operation demos for configurations that make sense for WAL
    const wal_configs = [_]DemoConfig{
        demo_configs[4], // wal_corruption
        demo_configs[5], // combined_failures
    };

    for (wal_configs) |config| {
        demoWALOperations(allocator, config) catch |err| {
            print("Error in WAL demo {s}: {}\n", .{ config.name, err });
        };
    }

    // Run deterministic behavior demo
    demoDeterministicBehavior(allocator) catch |err| {
        print("Error in deterministic demo: {}\n", .{err});
    };

    // Run recovery scenarios demo
    demoRecoveryScenarios(allocator) catch |err| {
        print("Error in recovery demo: {}\n", .{err});
    };

    print("\n=== Demo Complete ===\n", .{});
    print("The crash simulation system provides:\n", .{});
    print("1. Deterministic failure injection based on seeds\n", .{});
    print("2. Multiple corruption types (bit flips, header corruption, etc.)\n", .{});
    print("3. Various crash modes (before write, during write, etc.)\n", .{});
    print("4. Recovery testing and data integrity verification\n", .{});
    print("5. Statistics tracking for analysis\n", .{});
    print("\nThis enables comprehensive testing of crash recovery scenarios\n", .{});
    print("and helps ensure data durability and consistency.\n", .{});
}
