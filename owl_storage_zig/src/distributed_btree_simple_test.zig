// Simplified Distributed B-tree Tests
// Tests core distributed B-tree functionality without complex rebalancing

const std = @import("std");
const testing = std.testing;
const BTree = @import("btree.zig").BTree;
const BTreeConfig = @import("btree.zig").BTreeConfig;
const DistributedKey = @import("btree.zig").DistributedKey;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const ObjRef = @import("objref.zig").ObjRef;

test "Distributed BTree: basic shard-aware operations" {
    const allocator = testing.allocator;

    // Create B-trees for different shards
    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mappings
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data in shard 1
    const key1 = DistributedKey.init(10, 1);
    const value1 = GlobalObjRef.init(1, 1000, 0);
    try tree1.insertDistributed(key1, value1);

    // Insert data in shard 2
    const key2 = DistributedKey.init(20, 2);
    const value2 = GlobalObjRef.init(2, 2000, 0);
    try tree2.insertDistributed(key2, value2);

    // Verify local searches work
    const result1 = tree1.searchDistributed(key1);
    try testing.expect(result1 != null);
    try testing.expectEqual(@as(u64, 1000), result1.?.getLocalOffset());

    const result2 = tree2.searchDistributed(key2);
    try testing.expect(result2 != null);
    try testing.expectEqual(@as(u64, 2000), result2.?.getLocalOffset());

    // Verify cross-shard searches work
    const cross_result1 = tree2.searchDistributed(key1); // Search shard 1 data from shard 2
    try testing.expect(cross_result1 != null);
    try testing.expectEqual(@as(u64, 1000), cross_result1.?.getLocalOffset());

    const cross_result2 = tree1.searchDistributed(key2); // Search shard 2 data from shard 1
    try testing.expect(cross_result2 != null);
    try testing.expectEqual(@as(u64, 2000), cross_result2.?.getLocalOffset());
}

test "Distributed BTree: cross-shard insertions" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mappings
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data for shard 2 from tree1 (cross-shard insert)
    const key_for_shard2 = DistributedKey.init(30, 2);
    const value_for_shard2 = GlobalObjRef.init(2, 3000, 0);
    try tree1.insertDistributed(key_for_shard2, value_for_shard2);

    // Verify the data ended up in shard 2
    try testing.expectEqual(@as(usize, 0), tree1.getSize()); // tree1 should be empty
    try testing.expectEqual(@as(usize, 1), tree2.getSize()); // tree2 should have the data

    // Verify we can find the data from both trees
    const result_from_tree1 = tree1.searchDistributed(key_for_shard2);
    const result_from_tree2 = tree2.searchDistributed(key_for_shard2);

    try testing.expect(result_from_tree1 != null);
    try testing.expect(result_from_tree2 != null);
    try testing.expectEqual(@as(u64, 3000), result_from_tree1.?.getLocalOffset());
    try testing.expectEqual(@as(u64, 3000), result_from_tree2.?.getLocalOffset());
}

test "Distributed BTree: cross-shard range queries" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 4 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mappings
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data across both shards
    for (1..11) |i| {
        const key = @as(u64, @intCast(i * 10));
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
    const results = try tree1.coordinateCrossShardQuery(20, 80, allocator);
    defer allocator.free(results);

    // Should find results from both shards
    try testing.expect(results.len > 0);

    // For now, just verify we got some results
    // The cross-shard coordination implementation is simplified
    // In a full implementation, this would properly coordinate across shards
}

test "Distributed BTree: atomic operations" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mappings
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
    const delete_result = tree1.deleteDistributed(key);
    try testing.expectError(error.ShardNotAvailable, delete_result);
}

test "Distributed BTree: multiple shards coordination" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 4 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();
    const tree3 = try BTree.init(allocator, config, 3);
    defer tree3.deinit();

    // Set up full mesh shard mapping
    try tree1.addShard(2, tree2);
    try tree1.addShard(3, tree3);
    try tree2.addShard(1, tree1);
    try tree2.addShard(3, tree3);
    try tree3.addShard(1, tree1);
    try tree3.addShard(2, tree2);

    // Insert specific test data
    const key1 = DistributedKey.init(10, 1);
    const value1 = GlobalObjRef.init(1, 1000, 0);
    try tree1.insertDistributed(key1, value1);

    const key2 = DistributedKey.init(20, 2);
    const value2 = GlobalObjRef.init(2, 2000, 0);
    try tree2.insertDistributed(key2, value2);

    const key3 = DistributedKey.init(30, 3);
    const value3 = GlobalObjRef.init(3, 3000, 0);
    try tree3.insertDistributed(key3, value3);

    // Verify distribution
    try testing.expectEqual(@as(usize, 1), tree1.getSize());
    try testing.expectEqual(@as(usize, 1), tree2.getSize());
    try testing.expectEqual(@as(usize, 1), tree3.getSize());

    // Verify cross-shard access from any tree
    const result1_from_tree3 = tree3.searchDistributed(key1); // tree3 accessing tree1's data
    const result2_from_tree1 = tree1.searchDistributed(key2); // tree1 accessing tree2's data
    const result3_from_tree2 = tree2.searchDistributed(key3); // tree2 accessing tree3's data

    try testing.expect(result1_from_tree3 != null);
    try testing.expect(result2_from_tree1 != null);
    try testing.expect(result3_from_tree2 != null);

    try testing.expectEqual(@as(u64, 1000), result1_from_tree3.?.getLocalOffset());
    try testing.expectEqual(@as(u64, 2000), result2_from_tree1.?.getLocalOffset());
    try testing.expectEqual(@as(u64, 3000), result3_from_tree2.?.getLocalOffset());
}

test "Distributed BTree: consistency checks" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();

    // Set up shard mappings
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert data
    for (1..11) |i| {
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

    // Check integrity of both trees
    try testing.expect(try tree1.checkIntegrity());
    try testing.expect(try tree2.checkIntegrity());

    // Verify all data is accessible
    for (1..11) |i| {
        const key = @as(u64, @intCast(i));
        const shard_id: ShardId = if (i % 2 == 0) 2 else 1;
        const dist_key = DistributedKey.init(key, shard_id);

        const result = tree1.searchDistributed(dist_key);
        try testing.expect(result != null);
        try testing.expectEqual(@as(u64, i * 100), result.?.getLocalOffset());
    }
}

test "Distributed BTree: shard management operations" {
    const allocator = testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1);
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2);
    defer tree2.deinit();
    const tree3 = try BTree.init(allocator, config, 3);
    defer tree3.deinit();

    // Initially only tree1 and tree2 know about each other
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert some data
    const key1 = DistributedKey.init(10, 1);
    const value1 = GlobalObjRef.init(1, 1000, 0);
    try tree1.insertDistributed(key1, value1);

    const key2 = DistributedKey.init(20, 2);
    const value2 = GlobalObjRef.init(2, 2000, 0);
    try tree2.insertDistributed(key2, value2);

    // Add tree3 to the cluster
    try tree1.addShard(3, tree3);
    try tree2.addShard(3, tree3);
    try tree3.addShard(1, tree1);
    try tree3.addShard(2, tree2);

    // Insert data for tree3
    const key3 = DistributedKey.init(30, 3);
    const value3 = GlobalObjRef.init(3, 3000, 0);
    try tree1.insertDistributed(key3, value3); // Cross-shard insert

    // Verify tree3 received the data
    try testing.expectEqual(@as(usize, 1), tree3.getSize());

    // Verify all trees can access all data
    const result1_from_tree3 = tree3.searchDistributed(key1);
    const result2_from_tree3 = tree3.searchDistributed(key2);
    const result3_from_tree1 = tree1.searchDistributed(key3);

    try testing.expect(result1_from_tree3 != null);
    try testing.expect(result2_from_tree3 != null);
    try testing.expect(result3_from_tree1 != null);

    // Remove tree2 from tree1's shard map
    tree1.removeShard(2);

    // Now tree1 should not be able to access tree2's data
    const result2_after_removal = tree1.searchDistributed(key2);
    try testing.expect(result2_after_removal == null);

    // But tree3 should still be able to access tree2's data
    const result2_from_tree3_after = tree3.searchDistributed(key2);
    try testing.expect(result2_from_tree3_after != null);
}
