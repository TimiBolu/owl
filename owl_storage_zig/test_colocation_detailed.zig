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

    std.debug.print("\n=== Detailed Co-location Test ===\n\n", .{});

    // Allocate parent
    const parent = try arena.alloc(100);
    std.debug.print("Parent: offset={d}, page={d}, bump={d}, page_usage[0]={d}\n", .{
        parent.getOffset(),
        parent.pageId(),
        arena.bump.load(.seq_cst),
        arena.page_usage[0].load(.seq_cst),
    });

    // Allocate 100 children
    for (0..100) |i| {
        const child = try arena.allocNear(100, parent);
        const metrics = arena.colocationMetrics();
        std.debug.print("Child {d:2}: offset={d:4}, page={d}, bump={d:4}, page_usage[0]={d:4}, success={d:2}/{d:2}\n", .{
            i,
            child.getOffset(),
            child.pageId(),
            arena.bump.load(.seq_cst),
            arena.page_usage[0].load(.seq_cst),
            metrics.successful_colocations.load(.seq_cst),
            metrics.total_attempts.load(.seq_cst),
        });
    }

    const metrics = arena.colocationMetrics();
    std.debug.print("\nFinal metrics:\n", .{});
    std.debug.print("  Total attempts: {d}\n", .{metrics.total_attempts.load(.seq_cst)});
    std.debug.print("  Successful: {d}\n", .{metrics.successful_colocations.load(.seq_cst)});
    std.debug.print("  Failed: {d}\n", .{metrics.failed_colocations.load(.seq_cst)});
    std.debug.print("  Success rate: {d:.1}%\n", .{metrics.successRate()});
}
