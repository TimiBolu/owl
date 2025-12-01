//! Simple test to demonstrate deterministic allocation functionality

const std = @import("std");
const DeterministicArena = @import("src/deterministic_arena.zig").DeterministicArena;
const AllocationStrategy = @import("src/deterministic_arena.zig").AllocationStrategy;
const GCConfig = @import("src/deterministic_arena.zig").GCConfig;

test "deterministic allocation demo" {
    const allocator = std.testing.allocator;

    std.debug.print("\n=== Deterministic Allocation Demo ===\n", .{});

    // Create temporary file for demo
    var tmp_dir = std.testing.tmpDir(.{});
    defer tmp_dir.cleanup();

    const path = try tmp_dir.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const arena_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "demo_arena.dat" });
    defer allocator.free(arena_path);

    // Demo 1: Reproducible allocation patterns
    std.debug.print("\n1. Reproducible Allocation Patterns\n", .{});

    const seed: u64 = 42;
    const gc_config = GCConfig{};

    // Create two arenas with same seed
    var arena1 = try DeterministicArena.init(allocator, arena_path, 1024 * 1024, seed, .bump, gc_config);
    defer arena1.deinit();

    var arena2 = try DeterministicArena.init(allocator, arena_path, 1024 * 1024, seed, .bump, gc_config);
    defer arena2.deinit();

    // Perform same allocations
    const sizes = [_]u32{ 64, 128, 256, 512 };
    var all_match = true;

    for (sizes, 0..) |size, i| {
        const ref1 = try arena1.alloc(size);
        const ref2 = try arena2.alloc(size);

        const match = ref1.getOffset() == ref2.getOffset();
        if (!match) all_match = false;

        std.debug.print("   Allocation {}: size={d}, offset1={d}, offset2={d}, match={}\n", .{ i + 1, size, ref1.getOffset(), ref2.getOffset(), match });
    }

    std.debug.print("   Result: All allocations reproducible = {}\n", .{all_match});
    try std.testing.expect(all_match);

    // Demo 2: Different strategies produce different patterns
    std.debug.print("\n2. Different Allocation Strategies\n", .{});

    arena1.reset();
    arena2.reset();

    arena1.setStrategy(.bump);
    arena2.setStrategy(.random_placement);

    const ref1 = try arena1.alloc(128);
    const ref2 = try arena2.alloc(128);

    std.debug.print("   Bump strategy: offset={d}\n", .{ref1.getOffset()});
    std.debug.print("   Random strategy: offset={d}\n", .{ref2.getOffset()});
    std.debug.print("   Different strategies produce different results = {}\n", .{ref1.getOffset() != ref2.getOffset()});

    // Demo 3: Allocation logging
    std.debug.print("\n3. Allocation Logging\n", .{});

    arena1.reset();
    arena1.setLogging(true);

    _ = try arena1.alloc(64);
    _ = try arena1.alloc(128);
    _ = try arena1.alloc(256);

    const log = arena1.getAllocationLog();
    std.debug.print("   Logged {d} allocation events:\n", .{log.len});

    for (log) |event| {
        std.debug.print("     Seq={d}: size={d}, offset={d}, rng_state={d}\n", .{ event.sequence, event.size, event.offset, event.rng_state });
    }

    // Demo 4: Reset reproducibility
    std.debug.print("\n4. Reset Reproducibility\n", .{});

    const stats_before = arena1.stats();
    arena1.reset();
    const stats_after = arena1.stats();

    std.debug.print("   Before reset: used={d}, objects={d}, sequence={d}\n", .{ stats_before.used_size, stats_before.object_count, stats_before.allocation_sequence });
    std.debug.print("   After reset:  used={d}, objects={d}, sequence={d}\n", .{ stats_after.used_size, stats_after.object_count, stats_after.allocation_sequence });

    // Verify reset worked
    try std.testing.expectEqual(@as(usize, 0), stats_after.used_size);
    try std.testing.expectEqual(@as(u64, 0), stats_after.object_count);
    try std.testing.expectEqual(@as(u64, 0), stats_after.allocation_sequence);

    std.debug.print("\n=== Demo Complete ===\n", .{});
}
