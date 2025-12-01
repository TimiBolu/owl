//! Basic test to verify concurrency primitives work correctly

const std = @import("std");
const testing = std.testing;

// Import from the src directory
const concurrency = @import("src/concurrency_primitives.zig");
const AtomicRefCount = concurrency.AtomicRefCount;
const SharedObject = concurrency.SharedObject;
const AtomicObjectHeader = concurrency.AtomicObjectHeader;
const ObjectHeader = @import("src/object_header.zig").ObjectHeader;

test "Basic concurrency primitives functionality" {
    const allocator = testing.allocator;

    // Test atomic reference counting
    var ref_count = AtomicRefCount.init();
    try testing.expectEqual(@as(u32, 1), ref_count.getCount());

    _ = ref_count.acquire();
    try testing.expectEqual(@as(u32, 2), ref_count.getCount());

    _ = ref_count.release();
    try testing.expectEqual(@as(u32, 1), ref_count.getCount());

    // Test shared object
    const TestData = struct { value: i32 };
    var shared_obj = try SharedObject(TestData).create(allocator, .{ .value = 42 });
    defer shared_obj.release();

    try testing.expectEqual(@as(i32, 42), shared_obj.data.value);
    try testing.expectEqual(@as(u32, 1), shared_obj.getRefCount());

    // Test atomic object header operations
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    // Test atomic flag operations
    header.atomicSetDeleted(true);
    try testing.expect(header.isDeleted());

    header.atomicSetPinned(true);
    try testing.expect(header.isPinned());

    // Test atomic version increment
    const version = header.atomicIncrementVersion();
    try testing.expectEqual(@as(u32, 1), version);

    // Test atomic LSN operations
    header.atomicSetLSN(12345);
    try testing.expectEqual(@as(u64, 12345), header.atomicGetLSN());

    // Test combined version and LSN update
    const new_version = AtomicObjectHeader.atomicVersionAndLSNUpdate(&header, 99999);
    try testing.expectEqual(@as(u32, 2), new_version);
    try testing.expectEqual(@as(u64, 99999), header.atomicGetLSN());

    std.debug.print("All basic concurrency primitives tests passed!\n", .{});
}
