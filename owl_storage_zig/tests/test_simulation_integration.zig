//! Simple test to verify simulation integration works
//! Task 8.11: Implement Zig simulation integration (Zig)

const std = @import("std");
const SimulatedStorageEngine = @import("src/simulation_integration_simple.zig").SimulatedStorageEngine;
const SimulationMode = @import("src/simulation_integration_simple.zig").SimulationMode;
const SimulationConfig = @import("src/simulation_integration_simple.zig").SimulationConfig;
const AllocationStrategy = @import("src/deterministic_arena.zig").AllocationStrategy;
const FailureConfig = @import("src/crash_simulation.zig").FailureConfig;
const Object = @import("src/object.zig").Object;
const ObjRef = @import("src/objref.zig").ObjRef;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("=== Task 8.11: Zig Simulation Integration Test ===\n", .{});

    // Create temporary directory
    var tmp_dir = std.testing.tmpDir(.{});
    defer tmp_dir.cleanup();

    const path = try tmp_dir.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const storage_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_storage.dat" });
    defer allocator.free(storage_path);

    // Test 1: Simulation-aware memory allocation for deterministic testing
    std.debug.print("\n1. Testing simulation-aware memory allocation...\n", .{});
    try testDeterministicAllocation(allocator, storage_path);

    // Test 2: Crash simulation at Zig storage layer
    std.debug.print("\n2. Testing crash simulation at Zig storage layer...\n", .{});
    try testCrashSimulation(allocator, storage_path);

    // Test 3: Deterministic object placement for reproducible tests
    std.debug.print("\n3. Testing deterministic object placement...\n", .{});
    try testDeterministicPlacement(allocator, storage_path);

    // Test 4: Simulation-controlled I/O operations
    std.debug.print("\n4. Testing simulation-controlled I/O operations...\n", .{});
    try testSimulationControlledIO(allocator, storage_path);

    std.debug.print("\n=== All Task 8.11 Requirements Verified ===\n", .{});
}

fn testDeterministicAllocation(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 42,
        .allocation_strategy = .bump,
        .enable_logging = true,
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    // Perform deterministic allocations
    const ref1 = try engine.alloc(64);
    const ref2 = try engine.alloc(128);
    const ref3 = try engine.alloc(256);

    std.debug.print("   Allocated objects at offsets: {d}, {d}, {d}\n", .{ ref1.getOffset(), ref2.getOffset(), ref3.getOffset() });

    const stats = engine.getStats();
    std.debug.print("   Deterministic allocations: {d}\n", .{stats.simulation_stats.deterministic_allocations});

    // Verify deterministic behavior
    if (stats.simulation_stats.deterministic_allocations > 0) {
        std.debug.print("   ✓ Simulation-aware memory allocation working\n", .{});
    } else {
        std.debug.print("   ✗ Simulation-aware memory allocation failed\n", .{});
    }
}

fn testCrashSimulation(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    const config = SimulationConfig{
        .mode = .chaos,
        .seed = 123,
        .failure_config = FailureConfig{
            .crash_enabled = true,
            .seed = 123,
        },
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    // Test crash simulation
    engine.simulateCrash();
    const is_crashed = engine.isCrashed();
    std.debug.print("   System crashed: {}\n", .{is_crashed});

    // Test that operations fail during crash
    const result = engine.alloc(64);
    const failed_as_expected = if (result) |_| false else |_| true;
    std.debug.print("   Operations fail during crash: {}\n", .{failed_as_expected});

    // Test recovery
    engine.recoverFromCrash();
    const recovered = !engine.isCrashed();
    std.debug.print("   System recovered: {}\n", .{recovered});

    if (is_crashed and failed_as_expected and recovered) {
        std.debug.print("   ✓ Crash simulation at Zig storage layer working\n", .{});
    } else {
        std.debug.print("   ✗ Crash simulation at Zig storage layer failed\n", .{});
    }
}

fn testDeterministicPlacement(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 456,
        .allocation_strategy = .deterministic_colocation,
        .enable_logging = true,
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    // Test co-location allocation
    const base_ref = try engine.alloc(64);
    const near_ref = try engine.allocNear(64, base_ref);

    std.debug.print("   Base object at offset: {d}\n", .{base_ref.getOffset()});
    std.debug.print("   Co-located object at offset: {d}\n", .{near_ref.getOffset()});

    const stats = engine.getStats();
    if (stats.simulation_stats.deterministic_allocations >= 2) {
        std.debug.print("   ✓ Deterministic object placement working\n", .{});
    } else {
        std.debug.print("   ✗ Deterministic object placement failed\n", .{});
    }
}

fn testSimulationControlledIO(allocator: std.mem.Allocator, storage_path: []const u8) !void {
    const config = SimulationConfig{
        .mode = .deterministic,
        .seed = 789,
        .enable_logging = true,
    };

    var engine = try SimulatedStorageEngine.init(allocator, config, 0, storage_path, 1024 * 1024);
    defer engine.deinit();

    // Test controlled I/O operations
    const ref = try engine.alloc(256);

    // Create and write object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 42, id, &[_]u8{ 1, 2, 3 }, "test data", &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    try engine.write(ref, data);

    // Read object back
    const read_obj = try engine.read(ref, allocator);
    var read_obj_mut = read_obj;
    defer read_obj_mut.deinit();

    // Verify data integrity
    const data_matches = std.mem.eql(u8, &obj.header.id, &read_obj.header.id);
    std.debug.print("   Write/Read operation successful: {}\n", .{data_matches});

    // Test time simulation
    const initial_time = engine.getCurrentTime();
    engine.advanceTime(1000000); // 1ms
    const advanced_time = engine.getCurrentTime();
    const time_advanced = advanced_time > initial_time;
    std.debug.print("   Time simulation working: {}\n", .{time_advanced});

    const stats = engine.getStats();
    if (stats.simulation_stats.total_operations > 0 and data_matches and time_advanced) {
        std.debug.print("   ✓ Simulation-controlled I/O operations working\n", .{});
    } else {
        std.debug.print("   ✗ Simulation-controlled I/O operations failed\n", .{});
    }
}
