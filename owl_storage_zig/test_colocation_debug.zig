const std = @import("std");
const storage = @import("src/root.zig");
const Arena = storage.Arena;

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);
    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "debug.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 100 * 1024 * 1024);
    defer arena.deinit();

    std.debug.print("\n=== Co-location Debug ===\n\n", .{});

    // First allocation
    const parent = try arena.alloc(100);
    std.debug.print("Parent allocated:\n", .{});
    std.debug.print("  Offset: {d}\n", .{parent.getOffset()});
    std.debug.print("  Page: {d}\n", .{parent.pageId()});
    std.debug.print("  Bump pointer: {d}\n", .{arena.bump.load(.seq_cst)});
    std.debug.print("  Page 0 usage: {d}\n\n", .{arena.page_usage[0].load(.seq_cst)});

    // Try to allocate near
    const child1 = try arena.allocNear(100, parent);
    std.debug.print("Child 1 allocated:\n", .{});
    std.debug.print("  Offset: {d}\n", .{child1.getOffset()});
    std.debug.print("  Page: {d}\n", .{child1.pageId()});
    std.debug.print("  Bump pointer: {d}\n", .{arena.bump.load(.seq_cst)});
    std.debug.print("  Page 0 usage: {d}\n\n", .{arena.page_usage[0].load(.seq_cst)});

    const child2 = try arena.allocNear(100, parent);
    std.debug.print("Child 2 allocated:\n", .{});
    std.debug.print("  Offset: {d}\n", .{child2.getOffset()});
    std.debug.print("  Page: {d}\n", .{child2.pageId()});
    std.debug.print("  Bump pointer: {d}\n", .{arena.bump.load(.seq_cst)});
    std.debug.print("  Page 0 usage: {d}\n\n", .{arena.page_usage[0].load(.seq_cst)});

    const metrics = arena.colocationMetrics();
    std.debug.print("Metrics:\n", .{});
    std.debug.print("  Total attempts: {d}\n", .{metrics.total_attempts.load(.seq_cst)});
    std.debug.print("  Successful: {d}\n", .{metrics.successful_colocations.load(.seq_cst)});
    std.debug.print("  Failed: {d}\n", .{metrics.failed_colocations.load(.seq_cst)});
    std.debug.print("  Success rate: {d:.1}%\n\n", .{metrics.successRate()});
}
