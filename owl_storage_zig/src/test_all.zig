// Main test file that imports all module tests

test {
    _ = @import("objref.zig");
    _ = @import("object_header.zig");
    _ = @import("object.zig");
    _ = @import("arena.zig");
    _ = @import("c_api.zig");
}
