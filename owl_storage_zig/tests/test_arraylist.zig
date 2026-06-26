const std = @import("std");

test "ArrayList init" {
    const allocator = std.testing.allocator;
    var list = std.ArrayList(u32){};
    defer list.deinit(allocator);
    
    try list.append(allocator, 1);
    try std.testing.expectEqual(@as(usize, 1), list.items.len);
}
