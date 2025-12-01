//! Demonstration of deterministic allocation functionality
//!
//! This example shows how the DeterministicArena provides reproducible
//! allocation patterns for simulation testing.

const std = @import("std");
const owl_storage = @import("owl_storage");
const DeterministicArena = owl_storage.DeterministicArena;
const AllocationStrategy = owl_storage.AllocationStrategy;
const GCConfig = owl_storage.GCConfig;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("=== Deterministic Allocation Demo ===\n\n");

    // Create temporary directory for demo
    var tmp_dir = std.testing.tmpDir(.{});
    defer tmp_dir.cleanup();

    const path = try tmp_dir.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const arena_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "demo_arena.dat" });
    defer allocator.free(arena_path);

    // Demo 1: Reproducible allocation patterns
    try demoReproducibleAllocations(allocator, arena_path);

    // Demo 2: Different allocation strategies
    try demoAllocationStrategies(allocator, arena_path);

    // Demo 3: Deterministic garbage collection
    try demoDeterministicGC(allocator, arena_path);

    // Demo 4: Allocation logging and analysis
    try demoAllocationLogging(allocator, arena_path);

    std.debug.print("=== Demo Complete ===\n");
}

fn demoReproducibleAllocations(allocator: std.mem.Allocator, path: []const u8) !void {
    std.debug.print("1. Reproducible Allocation Patterns\n");
    std.debug.print("   Creating two arenas with same seed (42)...\n");

    const seed: u64 = 42;
    const gc_config = GCConfig{};

    // Create first arena
    var arena1 = try DeterministicArena.init(
        allocator,
        path,
        1024 * 1024, // 1MB
        seed,
        .bump,
        gc_config,
    );
    defer arena1.deinit();

    // Create second arena with same seed
    var arena2 = try DeterministicArena.init(
        allocator,
        path,
        1024 * 1024, // 1MB
        seed,
        .bump,
        gc_config,
    );
    defer arena2.deinit();

    // Perform same allocations on both arenas
    const sizes = [_]u32{ 64, 128, 256, 512, 1024 };
    var offsets1: [5]u64 = undefined;
    var offsets2: [5]u64 = undefined;

    for (sizes, 0..) |size, i| {
        const ref1 = try arena1.alloc(size);
        const ref2 = try arena2.alloc(size);

        offsets1[i] = ref1.getOffset();
        offsets2[i] = ref2.getOffset();

        std.debug.print("   Allocation {}: size={d}, offset1={d}, offset2={d}, match={}\n", .{ i + 1, size, offsets1[i], offsets2[i], offsets1[i] == offsets2[i] });
    }

    // Verify all offsets match
    var all_match = true;
    for (offsets1, offsets2) |o1, o2| {
        if (o1 != o2) all_match = false;
    }

    std.debug.print("   Result: All allocations match = {}\n\n", .{all_match});
}

fn demoAllocationStrategies(allocator: std.mem.Allocator, path: []const u8) !void {
    std.debug.print("2. Different Allocation Strategies\n");

    const seed: u64 = 123;
    const gc_config = GCConfig{};
    const strategies = [_]AllocationStrategy{ .bump, .random_placement, .fragmentation_sim };
    const strategy_names = [_][]const u8{ "Bump", "Random Placement", "Fragmentation Sim" };

    for (strategies, strategy_names) |strategy, name| {
        std.debug.print("   Testing {} strategy:\n", .{name});

        var arena = try DeterministicArena.init(
            allocator,
            path,
            1024 * 1024,
            seed,
            strategy,
            gc_config,
        );
        defer arena.deinit();

        // Allocate several objects
        for (0..5) |i| {
            const ref = try arena.alloc(128);
            std.debug.print("     Allocation {}: offset={d}\n", .{ i + 1, ref.getOffset() });
        }

        const stats = arena.stats();
        std.debug.print("     Final stats: used={d}, objects={d}, rng_state={d}\n", .{ stats.used_size, stats.object_count, stats.rng_state });
    }
    std.debug.print("\n");
}

fn demoDeterministicGC(allocator: std.mem.Allocator, path: []const u8) !void {
    std.debug.print("3. Deterministic Garbage Collection\n");

    const seed: u64 = 456;
    const gc_config = GCConfig{
        .enabled = true,
        .trigger_interval = 5, // Trigger after 5 allocations
        .collection_percentage = 50,
        .deterministic_selection = true,
    };

    var arena = try DeterministicArena.init(
        allocator,
        path,
        1024 * 1024,
        seed,
        .bump,
        gc_config,
    );
    defer arena.deinit();

    std.debug.print("   GC config: trigger_interval={d}, collection_percentage={d}%\n", .{ gc_config.trigger_interval, gc_config.collection_percentage });

    // Allocate objects to trigger GC
    for (0..12) |i| {
        const ref = try arena.alloc(64);
        const stats = arena.stats();

        std.debug.print("   Allocation {}: offset={d}, objects={d}, sequence={d}\n", .{ i + 1, ref.getOffset(), stats.object_count, stats.allocation_sequence });
    }

    const final_stats = arena.stats();
    std.debug.print("   Final: objects={d}, used={d} bytes\n\n", .{ final_stats.object_count, final_stats.used_size });
}

fn demoAllocationLogging(allocator: std.mem.Allocator, path: []const u8) !void {
    std.debug.print("4. Allocation Logging and Analysis\n");

    const seed: u64 = 789;
    const gc_config = GCConfig{};

    var arena = try DeterministicArena.init(
        allocator,
        path,
        1024 * 1024,
        seed,
        .random_placement,
        gc_config,
    );
    defer arena.deinit();

    // Enable logging
    arena.setLogging(true);
    std.debug.print("   Logging enabled, performing allocations...\n");

    // Perform various allocations
    const sizes = [_]u32{ 64, 128, 256, 64, 512, 128 };
    for (sizes) |size| {
        _ = try arena.alloc(size);
    }

    // Analyze allocation log
    const log = arena.getAllocationLog();
    std.debug.print("   Allocation log ({d} entries):\n", .{log.len});

    for (log) |event| {
        std.debug.print("     Seq={d}: size={d}, offset={d}, strategy={s}, rng_state={d}\n", .{ event.sequence, event.size, event.offset, @tagName(event.strategy), event.rng_state });
    }

    // Test reset functionality
    std.debug.print("   Testing reset functionality...\n");
    const stats_before = arena.stats();
    arena.reset();
    const stats_after = arena.stats();

    std.debug.print("   Before reset: used={d}, objects={d}, sequence={d}\n", .{ stats_before.used_size, stats_before.object_count, stats_before.allocation_sequence });
    std.debug.print("   After reset:  used={d}, objects={d}, sequence={d}\n\n", .{ stats_after.used_size, stats_after.object_count, stats_after.allocation_sequence });
}
