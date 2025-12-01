//! Integration tests for atomic operations across all storage components
//!
//! These tests verify that atomic operations work correctly across
//! arena allocation, WAL writing, and B-tree index updates.

const std = @import("std");
const Arena = @import("arena.zig").Arena;
const WAL = @import("wal.zig").WAL;
const BTree = @import("btree.zig").BTree;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const AtomicContext = @import("atomic_operations.zig").AtomicContext;
const atomicAllocateObject = @import("atomic_operations.zig").atomicAllocateObject;
const atomicDeallocateObject = @import("atomic_operations.zig").atomicDeallocateObject;
const atomicCheckpoint = @import("atomic_operations.zig").atomicCheckpoint;
const verifyAtomicConsistency = @import("atomic_operations.zig").verifyAtomicConsistency;

test "Atomic Integration: arena atomic allocation" {
    const allocator = std.testing.allocator;

    // Create temporary arena file
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 1024 * 1024, 0);
    defer arena.deinit();

    // Test atomic allocation
    const ref1 = try arena.atomicAlloc(100);
    const ref2 = try arena.atomicAlloc(200);

    try std.testing.expect(!ref1.isNull());
    try std.testing.expect(!ref2.isNull());
    try std.testing.expect(ref1.getOffset() != ref2.getOffset());

    // Test atomic free
    try arena.atomicFree(ref1);
    try arena.atomicFree(ref2);

    const stats = arena.stats();
    try std.testing.expectEqual(@as(u64, 0), stats.object_count);
}

test "Atomic Integration: WAL atomic append" {
    const allocator = std.testing.allocator;

    const test_dir = "test_atomic_wal";
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    // Create test WAL entry
    const data = "atomic test data";
    const objref = ObjRef.init(1024);
    var entry = try @import("wal.zig").WALEntry.init(allocator, wal.nextLSN(), .insert, objref, data);
    defer entry.deinit(allocator);

    // Test atomic append
    try wal.atomicAppend(&entry);

    const stats = wal.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.entry_count);
    try std.testing.expect(stats.last_lsn > 0);
}

test "Atomic Integration: B-tree atomic operations" {
    const allocator = std.testing.allocator;

    const config = @import("btree.zig").BTreeConfig{ .order = 3 };
    var tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    const key = @import("btree.zig").DistributedKey.init(42, 0);
    const value = GlobalObjRef.init(0, 1000, 0);

    // Test atomic insert
    try tree.atomicInsertDistributed(key, value);
    try std.testing.expectEqual(@as(usize, 1), tree.getSize());

    // Test atomic delete
    const deleted = try tree.atomicDeleteDistributed(key);
    try std.testing.expect(deleted);
    try std.testing.expectEqual(@as(usize, 0), tree.getSize());
}

test "Atomic Integration: object header atomic operations" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    // Test atomic flag operations
    header.atomicSetDeleted(true);
    try std.testing.expect(header.isDeleted());

    header.atomicSetOverflow(true);
    try std.testing.expect(header.hasOverflow());

    header.atomicSetPinned(true);
    try std.testing.expect(header.isPinned());

    // Test atomic version increment
    const version1 = header.atomicIncrementVersion();
    const version2 = header.atomicIncrementVersion();
    try std.testing.expectEqual(@as(u32, 1), version1);
    try std.testing.expectEqual(@as(u32, 2), version2);

    // Test atomic LSN operations
    header.atomicSetLSN(12345);
    const lsn = header.atomicGetLSN();
    try std.testing.expectEqual(@as(u64, 12345), lsn);
}

test "Atomic Integration: full storage stack" {
    const allocator = std.testing.allocator;

    // Create test directory
    const test_dir = "test_atomic_full_stack";
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create temporary arena file
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    // Initialize all components
    var arena = try Arena.init(allocator, file_path, 1024 * 1024, 0);
    defer arena.deinit();

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    const config = @import("btree.zig").BTreeConfig{ .order = 3 };
    var index = try BTree.init(allocator, config, 0);
    defer index.deinit();

    var ctx = AtomicContext.init(&arena, &wal, index, allocator, 0);

    // Create test objects
    const id1 = [_]u8{1} ** 16;
    var obj1 = try Object.init(allocator, 1, id1, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj1.deinit();

    const id2 = [_]u8{2} ** 16;
    var obj2 = try Object.init(allocator, 2, id2, &[_]u8{ 4, 5, 6 }, &[_]u8{}, &[_]ObjRef{});
    defer obj2.deinit();

    // Test atomic allocation of multiple objects
    const result1 = try atomicAllocateObject(&ctx, &obj1, 100);
    const result2 = try atomicAllocateObject(&ctx, &obj2, 200);

    // Verify both succeeded
    switch (result1) {
        .success => |success| {
            try std.testing.expect(!success.objref.isNull());
            try std.testing.expect(success.lsn > 0);
        },
        .failure => try std.testing.expect(false),
    }

    switch (result2) {
        .success => |success| {
            try std.testing.expect(!success.objref.isNull());
            try std.testing.expect(success.lsn > 0);
        },
        .failure => try std.testing.expect(false),
    }

    // Verify consistency
    const consistent = try verifyAtomicConsistency(&ctx);
    try std.testing.expect(consistent);

    // Verify objects can be found in index
    try std.testing.expect(index.search(100) != null);
    try std.testing.expect(index.search(200) != null);

    // Test atomic checkpoint
    const checkpoint_lsn = try atomicCheckpoint(&ctx);
    try std.testing.expect(checkpoint_lsn > 0);

    // Verify WAL stats
    const wal_stats = wal.getStats();
    try std.testing.expectEqual(checkpoint_lsn, wal_stats.last_checkpoint);
    try std.testing.expect(wal_stats.entry_count >= 3); // 2 inserts + 1 checkpoint

    // Test atomic deallocation
    const objref1 = switch (result1) {
        .success => |success| success.objref,
        .failure => return error.TestFailed,
    };

    const dealloc_result = try atomicDeallocateObject(&ctx, objref1, 100);
    switch (dealloc_result) {
        .success => |success| {
            try std.testing.expect(success.lsn > 0);
        },
        .failure => try std.testing.expect(false),
    }

    // Verify object is no longer in index
    try std.testing.expect(index.search(100) == null);
    try std.testing.expect(index.search(200) != null); // Other object still there

    // Final consistency check
    const final_consistent = try verifyAtomicConsistency(&ctx);

    // Debug information if consistency check fails
    if (!final_consistent) {
        const arena_stats = arena.stats();
        const index_size = index.getSize();
        std.debug.print("Arena object count: {}, Index size: {}\n", .{ arena_stats.object_count, index_size });
    }

    try std.testing.expect(final_consistent);
}

test "Atomic Integration: concurrent operations simulation" {
    const allocator = std.testing.allocator;

    // Create test directory
    const test_dir = "test_atomic_concurrent";
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create temporary arena file
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    // Initialize all components
    var arena = try Arena.init(allocator, file_path, 1024 * 1024, 0);
    defer arena.deinit();

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    const config = @import("btree.zig").BTreeConfig{ .order = 3 };
    var index = try BTree.init(allocator, config, 0);
    defer index.deinit();

    var ctx = AtomicContext.init(&arena, &wal, index, allocator, 0);

    // Simulate concurrent operations by performing many atomic operations
    var i: u32 = 0;
    while (i < 10) : (i += 1) {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{ @intCast(i), @intCast(i + 1), @intCast(i + 2) }, &[_]u8{}, &[_]ObjRef{});
        defer obj.deinit();

        const result = try atomicAllocateObject(&ctx, &obj, i * 10);
        switch (result) {
            .success => {},
            .failure => |err| {
                std.debug.print("Allocation {} failed: {}\n", .{ i, err });
                try std.testing.expect(false);
            },
        }
    }

    // Verify all objects were allocated
    try std.testing.expectEqual(@as(usize, 10), index.getSize());

    // Verify consistency after all operations
    const consistent = try verifyAtomicConsistency(&ctx);
    try std.testing.expect(consistent);

    // Test sequence numbers are monotonic
    const seq1 = ctx.nextSequence();
    const seq2 = ctx.nextSequence();
    const seq3 = ctx.nextSequence();

    try std.testing.expect(seq1 < seq2);
    try std.testing.expect(seq2 < seq3);
}

test "Atomic Integration: error handling and rollback" {
    const allocator = std.testing.allocator;

    // Create test directory
    const test_dir = "test_atomic_error_handling";
    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    // Create very small arena to trigger out-of-space errors
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 1024, 0); // Very small arena
    defer arena.deinit();

    var wal = try WAL.init(allocator, test_dir);
    defer wal.deinit();

    const config = @import("btree.zig").BTreeConfig{ .order = 3 };
    var index = try BTree.init(allocator, config, 0);
    defer index.deinit();

    var ctx = AtomicContext.init(&arena, &wal, index, allocator, 0);

    // Create a large object that should cause out-of-space error
    const id = [_]u8{1} ** 16;
    const large_data = [_]u8{0xFF} ** 2048; // Larger than arena
    var obj = try Object.init(allocator, 1, id, &large_data, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    // This should fail due to out of space
    const result = try atomicAllocateObject(&ctx, &obj, 42);
    switch (result) {
        .success => {
            // This shouldn't happen with such a large object in small arena
            try std.testing.expect(false);
        },
        .failure => |err| {
            // Should get OutOfSpace error
            try std.testing.expect(err == @import("atomic_operations.zig").AtomicError.OutOfSpace);
        },
    }

    // Verify no partial state was left behind
    try std.testing.expectEqual(@as(usize, 0), index.getSize());
    try std.testing.expect(index.search(42) == null);

    // System should still be consistent
    const consistent = try verifyAtomicConsistency(&ctx);
    try std.testing.expect(consistent);
}
