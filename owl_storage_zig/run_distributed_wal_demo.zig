//! Build script to run the distributed WAL demo

const std = @import("std");

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    // Import the demo directly
    _ = allocator;
    const demo = @import("examples/distributed_wal_demo.zig");
    try demo.main();
}
