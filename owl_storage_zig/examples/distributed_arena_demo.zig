// Distributed Arena Demo
//
// This example demonstrates the distributed arena functionality with GlobalObjRef
// support, showing how objects can be allocated and navigated across different shards.

const std = @import("std");
const owl = @import("storage");

const GlobalObjRef = owl.GlobalObjRef;
const ShardId = owl.ShardId;
const Arena = owl.Arena;
const Object = owl.Object;
const ObjRef = owl.ObjRef;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    std.debug.print("=== Distributed Arena Demo ===\n\n", .{});

    // Create two arenas representing different shards
    var tmp_dir1 = std.testing.tmpDir(.{});
    defer tmp_dir1.cleanup();
    var tmp_dir2 = std.testing.tmpDir(.{});
    defer tmp_dir2.cleanup();

    const path1 = try tmp_dir1.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path1);
    const path2 = try tmp_dir2.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path2);

    const file_path1 = try std.fs.path.join(allocator, &[_][]const u8{ path1, "shard1.dat" });
    defer allocator.free(file_path1);
    const file_path2 = try std.fs.path.join(allocator, &[_][]const u8{ path2, "shard2.dat" });
    defer allocator.free(file_path2);

    // Create arenas for different shards
    var arena1 = try Arena.init(allocator, file_path1, 1024 * 1024, 1); // Shard 1
    defer arena1.deinit();
    var arena2 = try Arena.init(allocator, file_path2, 1024 * 1024, 2); // Shard 2
    defer arena2.deinit();

    std.debug.print("Created two arenas:\n", .{});
    std.debug.print("  Arena 1: Shard ID = {d}\n", .{arena1.getShardId()});
    std.debug.print("  Arena 2: Shard ID = {d}\n", .{arena2.getShardId()});
    std.debug.print("\n", .{});

    // Create objects in different shards
    const id1 = [_]u8{1} ** 16;
    const id2 = [_]u8{2} ** 16;

    var obj1 = try Object.init(allocator, 1, id1, "User data for shard 1", &[_]u8{}, &[_]ObjRef{});
    defer obj1.deinit();
    var obj2 = try Object.init(allocator, 2, id2, "Order data for shard 2", &[_]u8{}, &[_]ObjRef{});
    defer obj2.deinit();

    // Serialize objects
    const data1 = try obj1.toBytes(allocator);
    defer allocator.free(data1);
    const data2 = try obj2.toBytes(allocator);
    defer allocator.free(data2);

    // Allocate and store objects using GlobalObjRef
    const global_ref1 = try arena1.allocGlobal(@intCast(data1.len));
    const global_ref2 = try arena2.allocGlobal(@intCast(data2.len));

    try arena1.writeGlobal(global_ref1, data1);
    try arena2.writeGlobal(global_ref2, data2);

    std.debug.print("Allocated objects:\n", .{});
    std.debug.print("  Object 1: {f}\n", .{global_ref1});
    std.debug.print("  Object 2: {f}\n", .{global_ref2});
    std.debug.print("\n", .{});

    // Demonstrate local navigation (O(1) performance)
    std.debug.print("=== Local Navigation (O(1) performance) ===\n", .{});

    // Navigate to local object in arena1
    const result1 = arena1.navigateGlobal(global_ref1, allocator);
    if (result1.isLocal()) {
        var retrieved_obj1 = result1.getLocal().?;
        defer retrieved_obj1.deinit();
        std.debug.print("✓ Local navigation successful in Arena 1\n", .{});
        std.debug.print("  Retrieved object type: {d}\n", .{retrieved_obj1.header.type_tag});
        std.debug.print("  Fixed data: \"{s}\"\n", .{retrieved_obj1.fixed});
    }

    // Navigate to local object in arena2
    const result2 = arena2.navigateGlobal(global_ref2, allocator);
    if (result2.isLocal()) {
        var retrieved_obj2 = result2.getLocal().?;
        defer retrieved_obj2.deinit();
        std.debug.print("✓ Local navigation successful in Arena 2\n", .{});
        std.debug.print("  Retrieved object type: {d}\n", .{retrieved_obj2.header.type_tag});
        std.debug.print("  Fixed data: \"{s}\"\n", .{retrieved_obj2.fixed});
    }
    std.debug.print("\n", .{});

    // Demonstrate cross-shard navigation detection
    std.debug.print("=== Cross-Shard Navigation Detection ===\n", .{});

    // Try to navigate to remote object from arena1
    const remote_result1 = arena1.navigateGlobal(global_ref2, allocator);
    if (remote_result1.isRemote()) {
        const remote_info = remote_result1.getRemoteInfo().?;
        std.debug.print("✓ Remote navigation detected from Arena 1\n", .{});
        std.debug.print("  Target shard: {d}\n", .{remote_info.shard_id});
        std.debug.print("  Target offset: {d}\n", .{remote_info.objref.getLocalOffset()});
        std.debug.print("  (In production, this would trigger a network call)\n", .{});
    }

    // Try to navigate to remote object from arena2
    const remote_result2 = arena2.navigateGlobal(global_ref1, allocator);
    if (remote_result2.isRemote()) {
        const remote_info = remote_result2.getRemoteInfo().?;
        std.debug.print("✓ Remote navigation detected from Arena 2\n", .{});
        std.debug.print("  Target shard: {d}\n", .{remote_info.shard_id});
        std.debug.print("  Target offset: {d}\n", .{remote_info.objref.getLocalOffset()});
        std.debug.print("  (In production, this would trigger a network call)\n", .{});
    }
    std.debug.print("\n", .{});

    // Demonstrate GlobalObjRef encoding/decoding
    std.debug.print("=== GlobalObjRef Encoding/Decoding ===\n", .{});

    const encoded = global_ref1.encode();
    std.debug.print("Original: {f}\n", .{global_ref1});
    std.debug.print("Encoded:  high=0x{x:0>16}, low=0x{x:0>16}\n", .{ encoded.high, encoded.low });

    const decoded = GlobalObjRef.decode(encoded.high, encoded.low);
    std.debug.print("Decoded:  {f}\n", .{decoded});

    const bytes = global_ref1.toBytes();
    const from_bytes = try GlobalObjRef.fromBytes(&bytes);
    std.debug.print("From bytes: {f}\n", .{from_bytes});
    std.debug.print("\n", .{});

    // Demonstrate co-location
    std.debug.print("=== Co-location Demonstration ===\n", .{});

    // Allocate objects near each other
    const ref_a = try arena1.allocGlobal(500);
    const ref_b = try arena1.allocGlobalNear(500, ref_a);

    std.debug.print("Object A: page={d}, slot={d}\n", .{ ref_a.pageId(), ref_a.slotId() });
    std.debug.print("Object B: page={d}, slot={d}\n", .{ ref_b.pageId(), ref_b.slotId() });

    if (ref_a.pageId() == ref_b.pageId()) {
        std.debug.print("✓ Objects successfully co-located on same page!\n", .{});
    } else {
        std.debug.print("○ Objects on different pages (still acceptable)\n", .{});
    }

    const metrics = arena1.colocationMetrics();
    std.debug.print("Co-location success rate: {d:.1}%\n", .{metrics.successRate()});
    std.debug.print("\n", .{});

    // Demonstrate error handling for remote operations
    std.debug.print("=== Error Handling for Remote Operations ===\n", .{});

    // Try to write to remote reference - should fail
    if (arena1.writeGlobal(global_ref2, data1)) |_| {
        std.debug.print("✗ Unexpected success writing to remote reference\n", .{});
    } else |err| {
        std.debug.print("✓ Correctly rejected write to remote reference: {}\n", .{err});
    }

    // Try to read from remote reference - should fail
    if (arena1.readGlobal(global_ref2, allocator)) |obj| {
        var mutable_obj = obj;
        mutable_obj.deinit();
        std.debug.print("✗ Unexpected success reading from remote reference\n", .{});
    } else |err| {
        std.debug.print("✓ Correctly rejected read from remote reference: {}\n", .{err});
    }

    // Try remote resolution - should indicate not implemented
    if (arena1.resolveRemoteRef(global_ref2, allocator)) |obj| {
        if (obj) |o| {
            var mutable_o = o;
            mutable_o.deinit();
        }
        std.debug.print("✗ Unexpected success in remote resolution\n", .{});
    } else |err| {
        std.debug.print("✓ Remote resolution correctly not implemented: {}\n", .{err});
    }
    std.debug.print("\n", .{});

    std.debug.print("=== Demo Complete ===\n", .{});
    std.debug.print("Key achievements:\n", .{});
    std.debug.print("  ✓ 128-bit GlobalObjRef implementation\n", .{});
    std.debug.print("  ✓ Shard-aware arena allocation\n", .{});
    std.debug.print("  ✓ O(1) local navigation performance\n", .{});
    std.debug.print("  ✓ Cross-shard reference detection\n", .{});
    std.debug.print("  ✓ Proper error handling for remote operations\n", .{});
    std.debug.print("  ✓ Co-location support for related objects\n", .{});
    std.debug.print("  ✓ Network-ready encoding/decoding\n", .{});
}
