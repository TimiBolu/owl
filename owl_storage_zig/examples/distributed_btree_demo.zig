// Distributed B-tree Demo
// Demonstrates the distributed B-tree index functionality including:
// - Shard-aware B-tree partitioning
// - Cross-shard index queries and updates
// - Distributed index rebalancing during shard changes
// - Consistent indexing across distributed nodes

const std = @import("std");
const print = std.debug.print;
const BTree = @import("btree").BTree;
const BTreeConfig = @import("btree").BTreeConfig;
const DistributedKey = @import("btree").DistributedKey;
const KeyRange = @import("btree").KeyRange;
const GlobalObjRef = @import("global_objref").GlobalObjRef;
const ShardId = @import("global_objref").ShardId;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    print("=== Distributed B-tree Index Demo ===\n\n");

    // Demo 1: Basic distributed operations
    try demoBasicDistributedOperations(allocator);

    // Demo 2: Cross-shard queries
    try demoCrossShardQueries(allocator);

    // Demo 3: Shard-aware partitioning
    try demoShardAwarePartitioning(allocator);

    // Demo 4: Multi-shard coordination
    try demoMultiShardCoordination(allocator);

    print("\n=== Demo Complete ===\n");
}

fn demoBasicDistributedOperations(allocator: std.mem.Allocator) !void {
    print("1. Basic Distributed Operations\n");
    print("================================\n");

    // Create B-trees for different shards
    const config = BTreeConfig{ .order = 4 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mappings (bidirectional)
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    print("Created 2 B-tree shards with bidirectional mapping\n");

    // Insert data in each shard
    const key1 = DistributedKey.init(100, 1);
    const value1 = GlobalObjRef.init(1, 10000, 0);
    try tree1.insertDistributed(key1, value1);
    print("Inserted key=100 in shard 1\n");

    const key2 = DistributedKey.init(200, 2);
    const value2 = GlobalObjRef.init(2, 20000, 0);
    try tree2.insertDistributed(key2, value2);
    print("Inserted key=200 in shard 2\n");

    // Demonstrate cross-shard insert
    const key_for_shard2 = DistributedKey.init(300, 2);
    const value_for_shard2 = GlobalObjRef.init(2, 30000, 0);
    try tree1.insertDistributed(key_for_shard2, value_for_shard2);
    print("Inserted key=300 for shard 2 from shard 1 (cross-shard insert)\n");

    // Verify sizes
    print("Shard 1 size: {}\n", .{tree1.getSize()});
    print("Shard 2 size: {}\n", .{tree2.getSize()});

    // Demonstrate cross-shard search
    const result = tree1.searchDistributed(key2); // Search shard 2 data from shard 1
    if (result) |r| {
        print("Cross-shard search: found key=200 with value offset={}\n", .{r.getLocalOffset()});
    }

    print("\n");
}

fn demoCrossShardQueries(allocator: std.mem.Allocator) !void {
    print("2. Cross-Shard Range Queries\n");
    print("=============================\n");

    const config = BTreeConfig{ .order = 4 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mappings
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data across both shards
    print("Inserting data across shards:\n");
    for (1..21) |i| {
        const key = @as(u64, @intCast(i * 10));
        const shard_id: ShardId = if (i % 2 == 0) 2 else 1;
        const dist_key = DistributedKey.init(key, shard_id);
        const value = GlobalObjRef.init(shard_id, i * 1000, 0);

        if (shard_id == 1) {
            try tree1.insertDistributed(dist_key, value);
            print("  Key {} -> Shard 1\n", .{key});
        } else {
            try tree2.insertDistributed(dist_key, value);
            print("  Key {} -> Shard 2\n", .{key});
        }
    }

    print("\nShard 1 size: {}, Shard 2 size: {}\n", .{ tree1.getSize(), tree2.getSize() });

    // Perform cross-shard range query
    print("\nPerforming cross-shard range query [50, 150]:\n");
    const results = try tree1.coordinateCrossShardQuery(50, 150, allocator);
    defer allocator.free(results);

    print("Found {} results:\n", .{results.len});
    for (results) |result| {
        print("  Shard {}, Offset {}\n", .{ result.getShardId(), result.getLocalOffset() });
    }

    print("\n");
}

fn demoShardAwarePartitioning(allocator: std.mem.Allocator) !void {
    print("3. Shard-Aware Partitioning\n");
    print("============================\n");

    const config = BTreeConfig{ .order = 4 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();
    const tree3 = try BTree.init(allocator, config, 3);
    defer tree3.deinit();

    // Set up shard mappings
    try tree1.addShard(2, tree2);
    try tree1.addShard(3, tree3);
    try tree2.addShard(1, tree1);
    try tree2.addShard(3, tree3);
    try tree3.addShard(1, tree1);
    try tree3.addShard(2, tree2);

    // Insert data randomly across shards
    print("Inserting data randomly across shards:\n");
    for (1..31) |i| {
        const key = @as(u64, @intCast(i));
        const shard_id: ShardId = @intCast((i % 3) + 1);
        const dist_key = DistributedKey.init(key, shard_id);
        const value = GlobalObjRef.init(shard_id, i * 100, 0);

        if (shard_id == 1) {
            try tree1.insertDistributed(dist_key, value);
        } else if (shard_id == 2) {
            try tree2.insertDistributed(dist_key, value);
        } else {
            try tree3.insertDistributed(dist_key, value);
        }
    }

    print("Initial distribution:\n");
    print("  Shard 1: {} entries\n", .{tree1.getSize()});
    print("  Shard 2: {} entries\n", .{tree2.getSize()});
    print("  Shard 3: {} entries\n", .{tree3.getSize()});

    // Define key ranges for partitioning
    const key_ranges = [_]KeyRange{
        KeyRange{ .start_key = 1, .end_key = 10, .target_shard = 1 },
        KeyRange{ .start_key = 11, .end_key = 20, .target_shard = 2 },
        KeyRange{ .start_key = 21, .end_key = 30, .target_shard = 3 },
    };

    print("\nApplying range-based partitioning:\n");
    print("  Keys 1-10  -> Shard 1\n");
    print("  Keys 11-20 -> Shard 2\n");
    print("  Keys 21-30 -> Shard 3\n");

    // Apply partitioning
    try tree1.partitionByKeyRange(&key_ranges, allocator);
    try tree2.partitionByKeyRange(&key_ranges, allocator);
    try tree3.partitionByKeyRange(&key_ranges, allocator);

    print("\nAfter partitioning:\n");
    print("  Shard 1: {} entries\n", .{tree1.getSize()});
    print("  Shard 2: {} entries\n", .{tree2.getSize()});
    print("  Shard 3: {} entries\n", .{tree3.getSize()});

    // Verify partitioning worked
    const test_key_shard1 = DistributedKey.init(5, 1);
    const test_key_shard2 = DistributedKey.init(15, 2);
    const test_key_shard3 = DistributedKey.init(25, 3);

    const result1 = tree1.searchDistributed(test_key_shard1);
    const result2 = tree2.searchDistributed(test_key_shard2);
    const result3 = tree3.searchDistributed(test_key_shard3);

    print("\nVerification:\n");
    print("  Key 5 in shard 1: {}\n", .{result1 != null});
    print("  Key 15 in shard 2: {}\n", .{result2 != null});
    print("  Key 25 in shard 3: {}\n", .{result3 != null});

    print("\n");
}

fn demoMultiShardCoordination(allocator: std.mem.Allocator) !void {
    print("4. Multi-Shard Coordination\n");
    print("============================\n");

    const config = BTreeConfig{ .order = 4 };
    const coordinator = try BTree.init(allocator, config, 1); // Coordinator shard
    defer coordinator.deinit();
    const follower1 = try BTree.init(allocator, config, 2);
    defer follower1.deinit();
    const follower2 = try BTree.init(allocator, config, 3);
    defer follower2.deinit();

    // Set up full mesh connectivity
    try coordinator.addShard(2, follower1);
    try coordinator.addShard(3, follower2);
    try follower1.addShard(1, coordinator);
    try follower1.addShard(3, follower2);
    try follower2.addShard(1, coordinator);
    try follower2.addShard(2, follower1);

    print("Set up 3-shard cluster with full mesh connectivity\n");

    // Insert data in coordinator
    print("\nInserting data in coordinator shard:\n");
    for (1..11) |i| {
        const key = @as(u64, @intCast(i * 10));
        const dist_key = DistributedKey.init(key, 1);
        const value = GlobalObjRef.init(1, i * 1000, 0);
        try coordinator.insertDistributed(dist_key, value);
        print("  Inserted key {}\n", .{key});
    }

    print("Coordinator size: {}\n", .{coordinator.getSize()});

    // Synchronize index structure across all nodes
    print("\nSynchronizing index structure across cluster...\n");
    try coordinator.synchronizeIndexStructure(1, allocator); // Coordinator broadcasts
    try follower1.synchronizeIndexStructure(1, allocator); // Follower receives
    try follower2.synchronizeIndexStructure(1, allocator); // Follower receives

    // Verify consistency across all nodes
    print("Checking integrity across all nodes:\n");
    const coord_integrity = try coordinator.checkIntegrity();
    const follower1_integrity = try follower1.checkIntegrity();
    const follower2_integrity = try follower2.checkIntegrity();

    print("  Coordinator integrity: {}\n", .{coord_integrity});
    print("  Follower 1 integrity: {}\n", .{follower1_integrity});
    print("  Follower 2 integrity: {}\n", .{follower2_integrity});

    // Test cross-shard access from all nodes
    const test_key = DistributedKey.init(50, 1);
    const coord_result = coordinator.searchDistributed(test_key);
    const follower1_result = follower1.searchDistributed(test_key);
    const follower2_result = follower2.searchDistributed(test_key);

    print("\nCross-shard access test (key=50):\n");
    print("  From coordinator: {}\n", .{coord_result != null});
    print("  From follower 1: {}\n", .{follower1_result != null});
    print("  From follower 2: {}\n", .{follower2_result != null});

    if (coord_result != null and follower1_result != null and follower2_result != null) {
        const all_equal = coord_result.?.getLocalOffset() == follower1_result.?.getLocalOffset() and
            coord_result.?.getLocalOffset() == follower2_result.?.getLocalOffset();
        print("  All results identical: {}\n", .{all_equal});
    }

    // Demonstrate shard management
    print("\nDemonstrating shard management:\n");
    print("  Removing follower 2 from coordinator's shard map\n");
    coordinator.removeShard(3);

    // Now coordinator should not be able to access follower2 directly
    // but follower1 should still be able to access both
    const coord_to_follower2 = coordinator.searchDistributed(DistributedKey.init(100, 3));
    const follower1_to_follower2 = follower1.searchDistributed(DistributedKey.init(100, 3));

    print("  Coordinator -> Follower 2: {}\n", .{coord_to_follower2 != null});
    print("  Follower 1 -> Follower 2: {}\n", .{follower1_to_follower2 != null});

    print("\n");
}
