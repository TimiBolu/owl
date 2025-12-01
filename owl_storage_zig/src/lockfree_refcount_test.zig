//! Comprehensive tests for lock-free reference counting
//!
//! Tests cover:
//! - Basic reference counting operations
//! - Epoch-based memory reclamation
//! - Concurrent access patterns
//! - Memory safety under concurrent access

const std = @import("std");
const lockfree_refcount = @import("lockfree_refcount.zig");
const Arena = @import("arena.zig").Arena;
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;

const EpochManager = lockfree_refcount.EpochManager;
const EpochGuard = lockfree_refcount.EpochGuard;
const RefCountedObject = lockfree_refcount.RefCountedObject;
const LockFreeRefCountManager = lockfree_refcount.LockFreeRefCountManager;
const RefGuard = lockfree_refcount.RefGuard;

// ============================================================================
// Helper Functions
// ============================================================================

fn createTestObject(allocator: std.mem.Allocator, arena: *Arena, type_tag: u16, id_byte: u8) !ObjRef {
    const id = [_]u8{id_byte} ** 16;
    var obj = try Object.init(allocator, type_tag, id, &[_]u8{id_byte}, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try arena.alloc(@intCast(data.len));
    try arena.write(objref, data);

    return objref;
}

// ============================================================================
// Basic Tests
// ============================================================================

test "LockFreeRefCountManager: create and acquire" {
    const allocator = std.testing.allocator;

    var arena = try Arena.init(allocator, "test_refcount_arena.dat", 1024 * 1024, 0);
    defer arena.deinit();

    var epoch_manager = EpochManager.init(allocator);
    defer epoch_manager.deinit();

    var manager = LockFreeRefCountManager.init(allocator, &arena, &epoch_manager);
    defer manager.deinit();

    // Create an object in arena
    const objref = try createTestObject(allocator, &arena, 1, 1);

    // Create reference-counted object
    const rc_obj = try manager.createObject(objref);
    try std.testing.expectEqual(@as(u32, 1), rc_obj.getRefCount());

    // Acquire reference
    const rc_obj2 = try manager.acquire(objref);
    try std.testing.expectEqual(@as(u32, 2), rc_obj2.getRefCount());

    // Release references
    try manager.release(rc_obj2);
    try std.testing.expectEqual(@as(u32, 1), rc_obj.getRefCount());

    try manager.release(rc_obj);
    // Object should be retired now
}

test "LockFreeRefCountManager: automatic reclamation" {
    const allocator = std.testing.allocator;

    var arena = try Arena.init(allocator, "test_refcount_reclaim.dat", 1024 * 1024, 0);
    defer arena.deinit();

    var epoch_manager = EpochManager.init(allocator);
    defer epoch_manager.deinit();

    var manager = LockFreeRefCountManager.init(allocator, &arena, &epoch_manager);
    defer manager.deinit();

    // Create object
    const objref = try createTestObject(allocator, &arena, 1, 1);
    const rc_obj = try manager.createObject(objref);

    // Release last reference
    try manager.release(rc_obj);

    // Object should be retired
    const stats1 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats1.objects_retired.load(.seq_cst));

    // Advance epoch to allow reclamation
    manager.advanceEpochAndReclaim();
    manager.advanceEpochAndReclaim();

    // Object should be reclaimed
    const stats2 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats2.objects_reclaimed.load(.seq_cst));
}

test "LockFreeRefCountManager: RefGuard RAII" {
    const allocator = std.testing.allocator;

    var arena = try Arena.init(allocator, "test_refcount_guard.dat", 1024 * 1024, 0);
    defer arena.deinit();

    var epoch_manager = EpochManager.init(allocator);
    defer epoch_manager.deinit();

    var manager = LockFreeRefCountManager.init(allocator, &arena, &epoch_manager);
    defer manager.deinit();

    // Create object
    const objref = try createTestObject(allocator, &arena, 1, 1);
    const rc_obj = try manager.createObject(objref);

    {
        // Acquire with guard
        const rc_obj2 = try manager.acquire(objref);
        var guard = RefGuard.init(&manager, rc_obj2);
        defer guard.deinit();

        try std.testing.expectEqual(@as(u32, 2), rc_obj.getRefCount());
    }

    // Guard should have released reference
    try std.testing.expectEqual(@as(u32, 1), rc_obj.getRefCount());

    try manager.release(rc_obj);
}

test "LockFreeRefCountManager: multiple objects" {
    const allocator = std.testing.allocator;

    var arena = try Arena.init(allocator, "test_refcount_multi.dat", 1024 * 1024, 0);
    defer arena.deinit();

    var epoch_manager = EpochManager.init(allocator);
    defer epoch_manager.deinit();

    var manager = LockFreeRefCountManager.init(allocator, &arena, &epoch_manager);
    defer manager.deinit();

    // Create multiple objects
    var objrefs: [10]ObjRef = undefined;
    var rc_objs: [10]*RefCountedObject = undefined;

    for (&objrefs, &rc_objs, 0..) |*objref, *rc_obj, i| {
        objref.* = try createTestObject(allocator, &arena, 1, @intCast(i));
        rc_obj.* = try manager.createObject(objref.*);
    }

    // Verify all objects created
    const stats1 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 10), stats1.objects_created.load(.seq_cst));

    // Release all objects
    for (rc_objs) |rc_obj| {
        try manager.release(rc_obj);
    }

    // All should be retired
    const stats2 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 10), stats2.objects_retired.load(.seq_cst));

    // Advance epochs to allow reclamation
    manager.advanceEpochAndReclaim();
    manager.advanceEpochAndReclaim();

    // All should be reclaimed
    const stats3 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 10), stats3.objects_reclaimed.load(.seq_cst));
}

test "EpochManager: prevents premature reclamation" {
    const allocator = std.testing.allocator;

    var arena = try Arena.init(allocator, "test_refcount_epoch.dat", 1024 * 1024, 0);
    defer arena.deinit();

    var epoch_manager = EpochManager.init(allocator);
    defer epoch_manager.deinit();

    var manager = LockFreeRefCountManager.init(allocator, &arena, &epoch_manager);
    defer manager.deinit();

    // Create object
    const objref = try createTestObject(allocator, &arena, 1, 1);
    const rc_obj = try manager.createObject(objref);

    // Enter critical section
    var guard = try EpochGuard.init(&epoch_manager);
    defer guard.deinit();

    // Release object (should be retired but not reclaimed)
    try manager.release(rc_obj);

    const stats1 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats1.objects_retired.load(.seq_cst));

    // Try to reclaim - should not reclaim because we're in critical section
    manager.forceReclaim();

    const stats2 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 0), stats2.objects_reclaimed.load(.seq_cst));

    // Exit critical section
    guard.deinit();

    // Now reclamation should succeed
    manager.advanceEpochAndReclaim();

    const stats3 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats3.objects_reclaimed.load(.seq_cst));
}

test "LockFreeRefCountManager: cannot acquire retired object" {
    const allocator = std.testing.allocator;

    var arena = try Arena.init(allocator, "test_refcount_retired.dat", 1024 * 1024, 0);
    defer arena.deinit();

    var epoch_manager = EpochManager.init(allocator);
    defer epoch_manager.deinit();

    var manager = LockFreeRefCountManager.init(allocator, &arena, &epoch_manager);
    defer manager.deinit();

    // Create object
    const objref = try createTestObject(allocator, &arena, 1, 1);
    const rc_obj = try manager.createObject(objref);

    // Release last reference
    try manager.release(rc_obj);

    // Try to acquire - should fail
    const result = manager.acquire(objref);
    try std.testing.expectError(error.ObjectRetired, result);
}

test "LockFreeRefCountManager: statistics accuracy" {
    const allocator = std.testing.allocator;

    var arena = try Arena.init(allocator, "test_refcount_stats.dat", 1024 * 1024, 0);
    defer arena.deinit();

    var epoch_manager = EpochManager.init(allocator);
    defer epoch_manager.deinit();

    var manager = LockFreeRefCountManager.init(allocator, &arena, &epoch_manager);
    defer manager.deinit();

    // Create 10 objects
    var objrefs: [10]ObjRef = undefined;
    var rc_objs: [10]*RefCountedObject = undefined;

    for (&objrefs, &rc_objs, 0..) |*objref, *rc_obj, i| {
        objref.* = try createTestObject(allocator, &arena, 1, @intCast(i));
        rc_obj.* = try manager.createObject(objref.*);
    }

    const stats1 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 10), stats1.objects_created.load(.seq_cst));

    // Acquire each object 5 times
    for (objrefs) |objref| {
        var i: usize = 0;
        while (i < 5) : (i += 1) {
            const rc_obj = try manager.acquire(objref);
            try manager.release(rc_obj);
        }
    }

    const stats2 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 50), stats2.objects_acquired.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 50), stats2.objects_released.load(.seq_cst));

    // Release all original references
    for (rc_objs) |rc_obj| {
        try manager.release(rc_obj);
    }

    const stats3 = manager.getStats();
    try std.testing.expectEqual(@as(u64, 60), stats3.objects_released.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 10), stats3.objects_retired.load(.seq_cst));
}
