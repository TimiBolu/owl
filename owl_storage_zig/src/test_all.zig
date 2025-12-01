// Main test file that imports all module tests

test {
    _ = @import("objref.zig");
    _ = @import("object_header.zig");
    _ = @import("object.zig");
    _ = @import("arena.zig");
    _ = @import("c_api.zig");
    _ = @import("btree.zig");
    _ = @import("gc_test.zig");
    _ = @import("lockfree_refcount.zig");
    _ = @import("lockfree_refcount_test.zig");
    _ = @import("memory_manager.zig");
    _ = @import("compression.zig");
    _ = @import("decompression_cache.zig");
}
