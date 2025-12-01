// Task 9.10: Memory Manager Demo
// Demonstrates custom allocators, memory pooling, compaction, and NUMA awareness

const std = @import("std");
const storage = @import("storage");
const MemoryManager = storage.MemoryManager;
const MemoryManagerConfig = storage.MemoryManagerConfig;
const AllocationStrategy = storage.MemoryAllocationStrategy;
const CompactionPolicy = storage.CompactionPolicy;
const NumaPolicy = storage.NumaPolicy;
const Arena = storage.Arena;
const Object = storage.Object;
const ObjRef = storage.ObjRef;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("\n=== Memory Manager Demo ===\n\n", .{});

    // Demo 1: Basic memory management with statistics
    try demoBasicMemoryManagement(allocator);

    // Demo 2: Memory compaction
    try demoMemoryCompaction(allocator);

    // Demo 3: NUMA-aware allocation
    try demoNumaAwareness(allocator);

    // Demo 4: Different allocation strategies
    try demoAllocationStrategies(allocator);

    // Demo 5: Compaction policies
    try demoCompactionPolicies(allocator);

    std.debug.print("\n=== Demo Complete ===\n", .{});
}

fn demoBasicMemoryManagement(allocator: std.mem.Allocator) !void {
    std.debug.print("--- Demo 1: Basic Memory Management ---\n", .{});

    var manager = try MemoryManager.init(allocator, .{});
    defer manager.deinit();

    // Simulate allocations
    std.debug.print("Allocating memory...\n", .{});
    manager.recordAllocation(1024);
    manager.recordAllocation(2048);
    manager.recordAllocation(4096);
    manager.recordAllocation(512);

    var stats = manager.getStats();
    std.debug.print("  Total allocated: {} bytes\n", .{stats.total_allocated});
    std.debug.print("  Total used: {} bytes\n", .{stats.total_used});
    std.debug.print("  Allocation count: {}\n", .{stats.allocation_count});
    std.debug.print("  Average allocation size: {d:.2} bytes\n", .{stats.avg_allocation_size});
    std.debug.print("  Peak usage: {} bytes\n", .{stats.peak_usage});

    // Simulate deallocations
    std.debug.print("\nDeallocating some memory...\n", .{});
    manager.recordDeallocation(2048);
    manager.recordDeallocation(512);

    stats = manager.getStats();
    std.debug.print("  Total used: {} bytes\n", .{stats.total_used});
    std.debug.print("  Total free: {} bytes\n", .{stats.total_free});
    std.debug.print("  Deallocation count: {}\n", .{stats.deallocation_count});
    std.debug.print("  Peak usage (unchanged): {} bytes\n\n", .{stats.peak_usage});
}

fn demoMemoryCompaction(allocator: std.mem.Allocator) !void {
    std.debug.print("--- Demo 2: Memory Compaction ---\n", .{});

    const config = MemoryManagerConfig{
        .compaction_policy = .threshold,
        .fragmentation_threshold = 0.3,
    };

    var manager = try MemoryManager.init(allocator, config);
    defer manager.deinit();

    // Create test arena
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "demo_arena.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 4 * 1024 * 1024, 0);
    defer arena.deinit();

    std.debug.print("Creating fragmentation...\n", .{});

    // Allocate objects
    var refs: [20]ObjRef = undefined;
    for (&refs, 0..) |*ref, i| {
        ref.* = try arena.alloc(1024);
        std.debug.print("  Allocated object {}: offset {}\n", .{ i, ref.getOffset() });
    }

    // Free every other object to create fragmentation
    var i: usize = 0;
    while (i < 20) : (i += 2) {
        try arena.free(refs[i]);
        std.debug.print("  Freed object {}\n", .{i});
    }

    const frag_before = manager.calculateFragmentation(&arena);
    std.debug.print("\nFragmentation before compaction: {d:.2}%\n", .{frag_before * 100.0});

    const should_compact = manager.shouldCompact(&arena);
    std.debug.print("Should compact: {}\n", .{should_compact});

    if (should_compact) {
        std.debug.print("\nPerforming compaction...\n", .{});
        try manager.compact(&arena);

        const frag_after = manager.calculateFragmentation(&arena);
        std.debug.print("Fragmentation after compaction: {d:.2}%\n", .{frag_after * 100.0});

        const stats = manager.getStats();
        std.debug.print("Compaction count: {}\n", .{stats.compaction_count});
    }

    std.debug.print("\n", .{});
}

fn demoNumaAwareness(allocator: std.mem.Allocator) !void {
    std.debug.print("--- Demo 3: NUMA-Aware Allocation ---\n", .{});

    // Try different NUMA policies
    const policies = [_]struct {
        policy: NumaPolicy,
        name: []const u8,
    }{
        .{ .policy = .local, .name = "Local" },
        .{ .policy = .preferred, .name = "Preferred" },
        .{ .policy = .interleave, .name = "Interleave" },
        .{ .policy = .bind, .name = "Bind" },
    };

    for (policies) |policy_info| {
        const config = MemoryManagerConfig{
            .numa_aware = true,
            .numa_policy = policy_info.policy,
            .preferred_numa_node = 0,
        };

        var manager = try MemoryManager.init(allocator, config);
        defer manager.deinit();

        std.debug.print("\nPolicy: {s}\n", .{policy_info.name});
        std.debug.print("  NUMA node count: {}\n", .{manager.numa_node_count});

        if (manager.numa_node_count > 0) {
            std.debug.print("  Allocations:\n", .{});
            var i: usize = 0;
            while (i < 5) : (i += 1) {
                const node = manager.getNumaNode();
                std.debug.print("    Allocation {}: NUMA node {}\n", .{ i, node });
            }
        } else {
            std.debug.print("  NUMA not available on this system\n", .{});
        }
    }

    std.debug.print("\n", .{});
}

fn demoAllocationStrategies(allocator: std.mem.Allocator) !void {
    std.debug.print("--- Demo 4: Allocation Strategies ---\n", .{});

    const strategies = [_]struct {
        strategy: AllocationStrategy,
        name: []const u8,
        description: []const u8,
    }{
        .{
            .strategy = .best_fit,
            .name = "Best-Fit",
            .description = "Minimizes fragmentation by finding smallest suitable block",
        },
        .{
            .strategy = .first_fit,
            .name = "First-Fit",
            .description = "Fast allocation by using first suitable block",
        },
        .{
            .strategy = .buddy,
            .name = "Buddy",
            .description = "Power-of-2 sizes for efficient splitting and coalescing",
        },
        .{
            .strategy = .slab,
            .name = "Slab",
            .description = "Fixed-size classes for predictable performance",
        },
    };

    for (strategies) |strategy_info| {
        const config = MemoryManagerConfig{
            .strategy = strategy_info.strategy,
        };

        var manager = try MemoryManager.init(allocator, config);
        defer manager.deinit();

        std.debug.print("\nStrategy: {s}\n", .{strategy_info.name});
        std.debug.print("  Description: {s}\n", .{strategy_info.description});
        std.debug.print("  Configuration: {s}\n", .{@tagName(manager.config.strategy)});
    }

    std.debug.print("\n", .{});
}

fn demoCompactionPolicies(allocator: std.mem.Allocator) !void {
    std.debug.print("--- Demo 5: Compaction Policies ---\n", .{});

    const policies = [_]struct {
        policy: CompactionPolicy,
        name: []const u8,
        description: []const u8,
    }{
        .{
            .policy = .never,
            .name = "Never",
            .description = "No automatic compaction",
        },
        .{
            .policy = .threshold,
            .name = "Threshold",
            .description = "Compact when fragmentation exceeds threshold",
        },
        .{
            .policy = .periodic,
            .name = "Periodic",
            .description = "Compact at regular intervals",
        },
        .{
            .policy = .on_demand,
            .name = "On-Demand",
            .description = "Compact only when explicitly requested",
        },
    };

    // Create test arena
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "demo_arena2.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 4 * 1024 * 1024, 0);
    defer arena.deinit();

    for (policies) |policy_info| {
        const config = MemoryManagerConfig{
            .compaction_policy = policy_info.policy,
            .fragmentation_threshold = 0.3,
            .compaction_interval_ms = 1000,
        };

        var manager = try MemoryManager.init(allocator, config);
        defer manager.deinit();

        std.debug.print("\nPolicy: {s}\n", .{policy_info.name});
        std.debug.print("  Description: {s}\n", .{policy_info.description});

        const should_compact = manager.shouldCompact(&arena);
        std.debug.print("  Should compact now: {}\n", .{should_compact});

        if (policy_info.policy == .threshold) {
            std.debug.print("  Fragmentation threshold: {d:.1}%\n", .{config.fragmentation_threshold * 100.0});
        } else if (policy_info.policy == .periodic) {
            std.debug.print("  Compaction interval: {}ms\n", .{config.compaction_interval_ms});
        }
    }

    std.debug.print("\n", .{});
}
