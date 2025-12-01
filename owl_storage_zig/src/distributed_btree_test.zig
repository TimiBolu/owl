// Distributed B-tree Tests
// Tests for distributed B-tree index functionality including:
// - Shard-aware B-tree partitioning
// - Cross-shard index queries and updates
// - Distributed index rebalancing during shard changes
// - Consistent indexing across distributed nodes

const std = @import("std");
const testing = std.testing;
const BTree = @import("btree.zig").BTree;
const BTreeConfig = @import("btree.zig").BTreeConfig;
const DistributedKey = @import("btree.zig").DistributedKey;
const KeyRange = @import("btree.zig").KeyRange;
const RebalancePlan = @import("btree.zig").RebalancePlan;
const ShardUpdate = @import("btree.zig").ShardUpdate;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const ObjRef = @import("objref.zig").ObjRef;

test "Distributed BTree: shard-aware partitioning" {
    const allocator = testing.allocator;

    // Create multiple B-trees for different shards
    const config = BTreeConfig{ .order = 3 };
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

    // Define key ranges for partitioning
    const key_ranges = [_]KeyRange{
        KeyRange{ .start_key = 1, .end_key = 33, .target_shard = 1 },
        KeyRange{ .start_key = 34, .end_key = 66, .target_shard = 2 },
        KeyRange{ .start_key = 67, .end_key = 100, .target_shard = 3 },
    };

    // Insert data across different shards initially
    for (1..101) |i| {
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

    // Verify initial distribution
    try testing.expect(tree1.getSize() > 0);
    try testing.expect(tree2.getSize() > 0);
    try testing.expect(tree3.getSize() > 0);

    // Perform partitioning based on key ranges
    try tree1.partitionByKeyRange(&key_ranges, allocator);
    try tree2.partitionByKeyRange(&key_ranges, allocator);
    try tree3.partitionByKeyRange(&key_ranges, allocator);

    // Verify partitioning worked correctly
    // Keys 1-33 should be in shard 1, 34-66 in shard 2, 67-100 in shard 3
    const key_10 = DistributedKey.init(10, 1);
    const result_10 = tree1.searchDistributed(key_10);
    try testing.expect(result_10 != null);

    const key_50 = DistributedKey.init(50, 2);
    const result_50 = tree2.searchDistributed(key_50);
    try testing.expect(result_50 != null);

    const key_80 = DistributedKey.init(80, 3);
    const result_80 = tree3.searchDistributed(key_80);
    try testing.expect(result_80 != null);
}

test "Distributed BTree: cross-shard index queries" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 4 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up bidirectional shard mapping
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data in both shards
    for (1..21) |i| {
        const key = @as(u64, @intCast(i));
        const shard_id: ShardId = if (i % 2 == 0) 2 else 1;
        const dist_key = DistributedKey.init(key, shard_id);
        const value = GlobalObjRef.init(shard_id, i * 100, 0);

        if (shard_id == 1) {
            try tree1.insertDistributed(dist_key, value);
        } else {
            try tree2.insertDistributed(dist_key, value);
        }
    }

    // Perform cross-shard range query
    const results = try tree1.coordinateCrossShardQuery(5, 15, allocator);
    defer allocator.free(results);

    // Should find results from both shards
    try testing.expect(results.len > 0);

    // Verify we got results from multiple shards
    var shard1_count: usize = 0;
    var shard2_count: usize = 0;

    for (results) |result| {
        if (result.getShardId() == 1) {
            shard1_count += 1;
        } else if (result.getShardId() == 2) {
            shard2_count += 1;
        }
    }

    try testing.expect(shard1_count > 0);
    try testing.expect(shard2_count > 0);
}

test "Distributed BTree: cross-shard updates" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mapping
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data in shard 1
    const key1 = DistributedKey.init(10, 1);
    const value1 = GlobalObjRef.init(1, 1000, 0);
    try tree1.insertDistributed(key1, value1);

    // Insert data that should go to shard 2 (cross-shard insert)
    const key2 = DistributedKey.init(20, 2);
    const value2 = GlobalObjRef.init(2, 2000, 0);
    try tree1.insertDistributed(key2, value2); // This should route to tree2

    // Verify both insertions worked
    const result1 = tree1.searchDistributed(key1);
    try testing.expect(result1 != null);
    try testing.expectEqual(@as(u64, 1000), result1.?.getLocalOffset());

    const result2 = tree1.searchDistributed(key2); // Cross-shard search
    try testing.expect(result2 != null);
    try testing.expectEqual(@as(u64, 2000), result2.?.getLocalOffset());

    // Verify the data is actually in the correct shard
    const direct_result2 = tree2.searchDistributed(key2);
    try testing.expect(direct_result2 != null);
    try testing.expectEqual(@as(u64, 2000), direct_result2.?.getLocalOffset());
}

test "Distributed BTree: index rebalancing during shard changes" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();
    const tree3 = try BTree.init(allocator, config, 3);
    defer tree3.deinit();

    // Set up initial shard mapping (only tree1 and tree2)
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data in both shards
    for (1..11) |i| {
        const key = @as(u64, @intCast(i));
        const shard_id: ShardId = if (i <= 5) 1 else 2;
        const dist_key = DistributedKey.init(key, shard_id);
        const value = GlobalObjRef.init(shard_id, i * 100, 0);

        if (shard_id == 1) {
            try tree1.insertDistributed(dist_key, value);
        } else {
            try tree2.insertDistributed(dist_key, value);
        }
    }

    // Verify initial state
    try testing.expectEqual(@as(usize, 5), tree1.getSize());
    try testing.expectEqual(@as(usize, 5), tree2.getSize());

    // Add tree3 to the cluster
    try tree1.addShard(3, tree3);
    try tree2.addShard(3, tree3);
    try tree3.addShard(1, tree1);
    try tree3.addShard(2, tree2);

    // Create rebalancing plan to move some data to tree3
    var shard_updates = [_]ShardUpdate{
        ShardUpdate{ .shard_id = 3, .operation = .add },
    };
    const rebalance_plan = RebalancePlan{ .shard_updates = &shard_updates };

    // Perform intelligent rebalancing
    try tree1.intelligentRebalance(rebalance_plan, allocator);
    try tree2.intelligentRebalance(rebalance_plan, allocator);

    // Verify rebalancing maintained data integrity
    try testing.expect(try tree1.checkIntegrity());
    try testing.expect(try tree2.checkIntegrity());
    try testing.expect(try tree3.checkIntegrity());

    // Verify all original data is still accessible
    for (1..11) |i| {
        const key = @as(u64, @intCast(i));
        const original_shard: ShardId = if (i <= 5) 1 else 2;
        const dist_key = DistributedKey.init(key, original_shard);

        // Should be able to find the data from any tree
        const result = tree1.searchDistributed(dist_key);
        try testing.expect(result != null);
    }
}

test "Distributed BTree: consistent indexing across nodes" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 4 };
    const coordinator = try BTree.init(allocator, config, 1); // Coordinator shard
    defer coordinator.deinit();
    const follower1 = try BTree.init(allocator, config, 2);
    defer follower1.deinit();
    const follower2 = try BTree.init(allocator, config, 3);
    defer follower2.deinit();

    // Set up shard mappings
    try coordinator.addShard(2, follower1);
    try coordinator.addShard(3, follower2);
    try follower1.addShard(1, coordinator);
    try follower1.addShard(3, follower2);
    try follower2.addShard(1, coordinator);
    try follower2.addShard(2, follower1);

    // Insert data in coordinator
    for (1..16) |i| {
        const key = @as(u64, @intCast(i));
        const dist_key = DistributedKey.init(key, 1);
        const value = GlobalObjRef.init(1, i * 100, 0);
        try coordinator.insertDistributed(dist_key, value);
    }

    // Synchronize index structure from coordinator to followers
    try coordinator.synchronizeIndexStructure(1, allocator); // Coordinator broadcasts
    try follower1.synchronizeIndexStructure(1, allocator); // Follower receives
    try follower2.synchronizeIndexStructure(1, allocator); // Follower receives

    // Verify consistency across all nodes
    try testing.expect(try coordinator.checkIntegrity());
    try testing.expect(try follower1.checkIntegrity());
    try testing.expect(try follower2.checkIntegrity());

    // Verify all nodes can access the same data
    const test_key = DistributedKey.init(10, 1);

    const coord_result = coordinator.searchDistributed(test_key);
    const follower1_result = follower1.searchDistributed(test_key);
    const follower2_result = follower2.searchDistributed(test_key);

    try testing.expect(coord_result != null);
    try testing.expect(follower1_result != null);
    try testing.expect(follower2_result != null);

    // Results should be identical
    try testing.expectEqual(coord_result.?.getLocalOffset(), follower1_result.?.getLocalOffset());
    try testing.expectEqual(coord_result.?.getLocalOffset(), follower2_result.?.getLocalOffset());
}

test "Distributed BTree: atomic operations across shards" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mapping
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Test atomic insert operations
    const key1 = DistributedKey.init(10, 1);
    const value1 = GlobalObjRef.init(1, 1000, 0);
    try tree1.atomicInsertDistributed(key1, value1);

    const key2 = DistributedKey.init(20, 2);
    const value2 = GlobalObjRef.init(2, 2000, 0);
    try tree1.atomicInsertDistributed(key2, value2);

    // Verify atomic operations maintained consistency
    try testing.expectEqual(@as(usize, 1), tree1.getSize());
    try testing.expectEqual(@as(usize, 1), tree2.getSize());

    // Test atomic delete operations
    const deleted1 = try tree1.atomicDeleteDistributed(key1);
    try testing.expect(deleted1);
    try testing.expectEqual(@as(usize, 0), tree1.getSize());

    const deleted2 = try tree1.atomicDeleteDistributed(key2); // Cross-shard delete
    try testing.expect(deleted2);
    try testing.expectEqual(@as(usize, 0), tree2.getSize());
}

test "Distributed BTree: large scale distributed operations" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 8 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();
    const tree3 = try BTree.init(allocator, config, 3);
    defer tree3.deinit();
    const tree4 = try BTree.init(allocator, config, 4);
    defer tree4.deinit();

    // Set up full mesh shard mapping
    const trees = [_]*BTree{ tree1, tree2, tree3, tree4 };
    for (trees, 1..) |tree, i| {
        for (trees, 1..) |other_tree, j| {
            if (i != j) {
                try tree.addShard(@intCast(j), other_tree);
            }
        }
    }

    // Insert large dataset across all shards
    for (1..1001) |i| {
        const key = @as(u64, @intCast(i));
        const shard_id: ShardId = @intCast((i % 4) + 1);
        const dist_key = DistributedKey.init(key, shard_id);
        const value = GlobalObjRef.init(shard_id, i * 100, 0);

        const target_tree = trees[shard_id - 1];
        try target_tree.insertDistributed(dist_key, value);
    }

    // Verify distribution
    var total_size: usize = 0;
    for (trees) |tree| {
        total_size += tree.getSize();
    }
    try testing.expectEqual(@as(usize, 1000), total_size);

    // Perform large cross-shard range query
    const results = try tree1.coordinateCrossShardQuery(100, 900, allocator);
    defer allocator.free(results);

    try testing.expect(results.len > 0);
    try testing.expect(results.len <= 801); // Should find keys 100-900

    // Verify results span multiple shards
    var shards_found = std.AutoHashMap(ShardId, bool).init(allocator);
    defer shards_found.deinit();

    for (results) |result| {
        try shards_found.put(result.getShardId(), true);
    }

    try testing.expect(shards_found.count() > 1); // Should span multiple shards
}

test "Distributed BTree: error handling for unavailable shards" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();

    // Try to insert data for a shard that doesn't exist
    const key = DistributedKey.init(10, 999); // Non-existent shard
    const value = GlobalObjRef.init(999, 1000, 0);

    const result = tree1.insertDistributed(key, value);
    try testing.expectError(error.ShardNotAvailable, result);

    // Try to search for data in non-existent shard
    const search_result = tree1.searchDistributed(key);
    try testing.expect(search_result == null);

    // Try to delete from non-existent shard
    const delete_result = try tree1.deleteDistributed(key);
    try testing.expect(!delete_result);
}

test "Distributed BTree: concurrent access patterns" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 4 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mapping
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Simulate concurrent operations (simplified - in real test would use threads)
    for (1..51) |i| {
        const key = @as(u64, @intCast(i));
        const shard_id: ShardId = if (i % 2 == 0) 2 else 1;
        const dist_key = DistributedKey.init(key, shard_id);
        const value = GlobalObjRef.init(shard_id, i * 100, 0);

        // Alternate between trees for insertion
        if (i % 2 == 0) {
            try tree1.insertDistributed(dist_key, value);
        } else {
            try tree2.insertDistributed(dist_key, value);
        }
    }

    // Verify all data is accessible and consistent
    for (1..51) |i| {
        const key = @as(u64, @intCast(i));
        const shard_id: ShardId = if (i % 2 == 0) 2 else 1;
        const dist_key = DistributedKey.init(key, shard_id);

        const result1 = tree1.searchDistributed(dist_key);
        const result2 = tree2.searchDistributed(dist_key);

        try testing.expect(result1 != null);
        try testing.expect(result2 != null);
        try testing.expectEqual(result1.?.getLocalOffset(), result2.?.getLocalOffset());
    }

    // Verify integrity of both trees
    try testing.expect(try tree1.checkIntegrity());
    try testing.expect(try tree2.checkIntegrity());
}
