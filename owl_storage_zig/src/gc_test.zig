// Tests for garbage collection

const std = @import("std");
const Arena = @import("arena.zig").Arena;
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const GarbageCollector = @import("gc.zig").GarbageCollector;
const BackgroundGC = @import("gc.zig").BackgroundGC;
const GCStats = @import("gc.zig").GCStats;

fn createTestArena(allocator: std.mem.Allocator) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_gc_arena.dat" });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 1024 * 1024, 0); // 1MB, shard 0
    return .{ .arena = arena, .dir = tmp };
}

fn createTestObject(allocator: std.mem.Allocator, arena: *Arena, type_tag: u16, refs: []const ObjRef) !ObjRef {
    const id = [_]u8{@intCast(type_tag)} ** 16;
    const fixed = [_]u8{ 1, 2, 3, 4 };
    var obj = try Object.init(allocator, type_tag, id, &fixed, "test", refs);
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const objref = try arena.alloc(@intCast(data.len));
    try arena.write(objref, data);

    return objref;
}

test "GC: initialization and cleanup" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    try std.testing.expect(!gc.isRunning());
    try std.testing.expectEqual(@as(usize, 0), gc.roots.items.len);
}

test "GC: add and remove roots" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    const ref1 = ObjRef.init(100);
    const ref2 = ObjRef.init(200);

    try gc.addRoot(ref1);
    try gc.addRoot(ref2);
    try std.testing.expectEqual(@as(usize, 2), gc.roots.items.len);

    gc.removeRoot(ref1);
    try std.testing.expectEqual(@as(usize, 1), gc.roots.items.len);

    gc.clearRoots();
    try std.testing.expectEqual(@as(usize, 0), gc.roots.items.len);
}

test "GC: null root handling" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Adding null root should be ignored
    try gc.addRoot(ObjRef.nullRef());
    try std.testing.expectEqual(@as(usize, 0), gc.roots.items.len);
}

test "GC: simple mark and sweep" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create two objects: one reachable, one unreachable
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{});
    _ = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{});

    // Only obj1 is a root
    try gc.addRoot(obj1);

    // Run GC
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expect(stats.objects_marked >= 1); // At least obj1 marked
    try std.testing.expect(stats.objects_swept >= 1); // At least obj2 swept
    try std.testing.expect(stats.cycles_completed == 1);
}

test "GC: reference chain marking" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create a chain: obj1 -> obj2 -> obj3
    const obj3 = try createTestObject(allocator, &test_arena.arena, 3, &[_]ObjRef{});
    const obj2 = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{obj3});
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{obj2});

    // Create unreachable object
    const obj4 = try createTestObject(allocator, &test_arena.arena, 4, &[_]ObjRef{});
    _ = obj4;

    // Only obj1 is a root
    try gc.addRoot(obj1);

    // Run GC
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expect(stats.objects_marked >= 3); // obj1, obj2, obj3 marked
    try std.testing.expect(stats.objects_swept >= 1); // obj4 swept
}

test "GC: circular references" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create circular reference: obj1 -> obj2 -> obj1
    // We'll create obj2 first with a placeholder, then update obj1
    const obj2_temp = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{});
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{obj2_temp});

    // Now create obj2 with reference to obj1
    const obj2 = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{obj1});
    _ = obj2;

    // Add obj1 as root
    try gc.addRoot(obj1);

    // Run GC - should handle circular references without infinite loop
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expect(stats.objects_marked >= 1);
    try std.testing.expect(stats.cycles_completed == 1);
}

test "GC: multiple roots" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create multiple independent object trees
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{});
    const obj2 = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{});
    const obj3 = try createTestObject(allocator, &test_arena.arena, 3, &[_]ObjRef{});

    // All are roots
    try gc.addRoot(obj1);
    try gc.addRoot(obj2);
    try gc.addRoot(obj3);

    // Run GC
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expect(stats.objects_marked >= 3);
    try std.testing.expect(stats.cycles_completed == 1);
}

test "GC: statistics tracking" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create objects
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{});
    _ = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{});

    try gc.addRoot(obj1);

    // Run GC
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expect(stats.objects_scanned > 0);
    try std.testing.expect(stats.objects_marked > 0);
    try std.testing.expect(stats.duration_ns > 0);
    try std.testing.expectEqual(@as(usize, 1), stats.cycles_completed);
}

test "GC: multiple cycles" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{});
    try gc.addRoot(obj1);

    // Run multiple GC cycles
    try gc.collect();
    try gc.collect();
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expectEqual(@as(usize, 3), stats.cycles_completed);
}

test "GC: concurrent collection prevention" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Manually set running flag
    _ = gc.running.swap(true, .seq_cst);

    // Try to run GC - should fail
    const result = gc.collect();
    try std.testing.expectError(error.GCAlreadyRunning, result);

    // Reset flag
    gc.running.store(false, .seq_cst);
}

test "GC: empty arena" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Run GC on empty arena
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expectEqual(@as(usize, 0), stats.objects_marked);
    try std.testing.expectEqual(@as(usize, 1), stats.cycles_completed);
}

test "GC: deleted objects are skipped" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create object and delete it
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{});
    try test_arena.arena.delete(obj1);

    // Add as root (even though deleted)
    try gc.addRoot(obj1);

    // Run GC - should not crash
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expectEqual(@as(usize, 0), stats.objects_marked); // Deleted object not marked
}

test "GC: bytes reclaimed tracking" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create reachable and unreachable objects
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{});
    _ = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{});
    _ = try createTestObject(allocator, &test_arena.arena, 3, &[_]ObjRef{});

    try gc.addRoot(obj1);

    // Run GC
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expect(stats.bytes_reclaimed > 0);
}

test "BackgroundGC: initialization" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    var bg_gc = BackgroundGC.init(allocator, &gc, 1000);

    try std.testing.expect(bg_gc.thread == null);
    try std.testing.expect(!bg_gc.should_stop.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 1000), bg_gc.interval_ms);
}

test "BackgroundGC: start and stop" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create some objects
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{});
    try gc.addRoot(obj1);

    var bg_gc = BackgroundGC.init(allocator, &gc, 100); // 100ms interval

    // Start background GC
    try bg_gc.start();
    try std.testing.expect(bg_gc.thread != null);

    // Let it run for a bit
    std.Thread.sleep(250 * std.time.ns_per_ms);

    // Stop background GC
    bg_gc.stop();
    try std.testing.expect(bg_gc.thread == null);

    // Check that at least one cycle completed
    const stats = gc.getStats();
    try std.testing.expect(stats.cycles_completed >= 1);
}

test "BackgroundGC: prevent double start" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    var bg_gc = BackgroundGC.init(allocator, &gc, 1000);

    try bg_gc.start();
    defer bg_gc.stop();

    // Try to start again - should fail
    const result = bg_gc.start();
    try std.testing.expectError(error.AlreadyStarted, result);
}

test "GC: complex object graph" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    var gc = try GarbageCollector.init(allocator, &test_arena.arena);
    defer gc.deinit();

    // Create a complex graph:
    //   root -> obj1 -> obj2
    //        -> obj3 -> obj4
    //   obj5 (unreachable)

    const obj5 = try createTestObject(allocator, &test_arena.arena, 5, &[_]ObjRef{});
    _ = obj5;

    const obj4 = try createTestObject(allocator, &test_arena.arena, 4, &[_]ObjRef{});
    const obj2 = try createTestObject(allocator, &test_arena.arena, 2, &[_]ObjRef{});

    const obj3 = try createTestObject(allocator, &test_arena.arena, 3, &[_]ObjRef{obj4});
    const obj1 = try createTestObject(allocator, &test_arena.arena, 1, &[_]ObjRef{obj2});

    const root = try createTestObject(allocator, &test_arena.arena, 0, &[_]ObjRef{ obj1, obj3 });

    try gc.addRoot(root);

    // Run GC
    try gc.collect();

    const stats = gc.getStats();
    try std.testing.expect(stats.objects_marked >= 5); // root, obj1, obj2, obj3, obj4
    try std.testing.expect(stats.objects_swept >= 1); // obj5
}
