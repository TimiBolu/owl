// Task 2.1: C API - Core Operations
// C API exports for Rust integration

const std = @import("std");
const Arena = @import("arena.zig").Arena;
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;

// C-compatible error codes
pub const OWL_OK: c_int = 0;
pub const OWL_ERR_OUT_OF_MEMORY: c_int = -1;
pub const OWL_ERR_INVALID_ARENA: c_int = -2;
pub const OWL_ERR_INVALID_REF: c_int = -3;
pub const OWL_ERR_DATA_TOO_SMALL: c_int = -4;
pub const OWL_ERR_OUT_OF_BOUNDS: c_int = -5;
pub const OWL_ERR_OBJECT_DELETED: c_int = -6;

// Global allocator for C API (uses libc allocator)
var gpa = std.heap.c_allocator;

/// Create a new arena
/// Returns null on failure
export fn owl_arena_create(path: [*:0]const u8, size: usize) ?*Arena {
    const path_slice = std.mem.span(path);
    const arena = gpa.create(Arena) catch return null;
    arena.* = Arena.init(gpa, path_slice, size) catch {
        gpa.destroy(arena);
        return null;
    };
    return arena;
}

/// Destroy an arena
export fn owl_arena_destroy(arena: ?*Arena) void {
    if (arena) |a| {
        a.deinit();
        gpa.destroy(a);
    }
}

/// Allocate an object
export fn owl_arena_alloc(
    arena: ?*Arena,
    size: u32,
    out_ref: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = a.alloc(size) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
        };
    };
    if (out_ref) |ptr| {
        ptr.* = ref.getOffset();
    }
    return OWL_OK;
}

/// Allocate an object near another object (for co-location)
export fn owl_arena_alloc_near(
    arena: ?*Arena,
    size: u32,
    near: u64,
    out_ref: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const near_ref: ?ObjRef = if (near == 0) null else ObjRef.init(near);
    const ref = a.allocNear(size, near_ref) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
        };
    };
    if (out_ref) |ptr| {
        ptr.* = ref.getOffset();
    }
    return OWL_OK;
}

/// Write data to an object
export fn owl_arena_write(
    arena: ?*Arena,
    obj_ref: u64,
    data: ?[*]const u8,
    data_len: usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = ObjRef.init(obj_ref);
    const data_slice = (data orelse return OWL_ERR_INVALID_REF)[0..data_len];
    a.write(ref, data_slice) catch |err| {
        return switch (err) {
            error.InvalidReference => OWL_ERR_INVALID_REF,
            error.DataTooSmall => OWL_ERR_DATA_TOO_SMALL,
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
        };
    };
    return OWL_OK;
}

/// Read an object from the arena
/// Note: Caller must free the returned object data
export fn owl_arena_read(
    arena: ?*Arena,
    obj_ref: u64,
    buffer: ?[*]u8,
    buffer_len: usize,
    out_bytes_read: ?*usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = ObjRef.init(obj_ref);
    const buf = (buffer orelse return OWL_ERR_INVALID_REF)[0..buffer_len];

    // Read object
    var obj = a.read(ref, gpa) catch |err| {
        return switch (err) {
            error.InvalidReference => OWL_ERR_INVALID_REF,
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
            error.ObjectDeleted => OWL_ERR_OBJECT_DELETED,
            else => OWL_ERR_INVALID_ARENA,
        };
    };
    defer obj.deinit();

    // Serialize to buffer
    const bytes = obj.toBytes(gpa) catch return OWL_ERR_INVALID_ARENA;
    defer gpa.free(bytes);

    const copy_len = @min(buf.len, bytes.len);
    @memcpy(buf[0..copy_len], bytes[0..copy_len]);

    if (out_bytes_read) |ptr| {
        ptr.* = copy_len;
    }

    return OWL_OK;
}

/// Delete an object
export fn owl_arena_delete(
    arena: ?*Arena,
    obj_ref: u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = ObjRef.init(obj_ref);
    a.delete(ref) catch |err| {
        return switch (err) {
            error.InvalidReference => OWL_ERR_INVALID_REF,
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
            else => OWL_ERR_INVALID_ARENA,
        };
    };
    return OWL_OK;
}

/// Get arena statistics
export fn owl_arena_stats(
    arena: ?*Arena,
    out_total: ?*usize,
    out_used: ?*usize,
    out_free: ?*usize,
    out_object_count: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const arena_stats = a.stats();
    if (out_total) |ptr| ptr.* = arena_stats.total_size;
    if (out_used) |ptr| ptr.* = arena_stats.used_size;
    if (out_free) |ptr| ptr.* = arena_stats.free_size;
    if (out_object_count) |ptr| ptr.* = arena_stats.object_count;
    return OWL_OK;
}

/// Get co-location metrics
export fn owl_arena_colocation_metrics(
    arena: ?*Arena,
    out_total_attempts: ?*usize,
    out_successful: ?*usize,
    out_failed: ?*usize,
    out_success_rate: ?*f64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const metrics = a.colocationMetrics();
    if (out_total_attempts) |ptr| ptr.* = metrics.total_attempts.load(.seq_cst);
    if (out_successful) |ptr| ptr.* = metrics.successful_colocations.load(.seq_cst);
    if (out_failed) |ptr| ptr.* = metrics.failed_colocations.load(.seq_cst);
    if (out_success_rate) |ptr| ptr.* = metrics.successRate();
    return OWL_OK;
}

/// Get the base pointer for swizzling (advanced usage)
export fn owl_arena_base_ptr(arena: ?*Arena) ?[*]u8 {
    const a = arena orelse return null;
    return a.basePtr();
}

// ============================================================================
// Tests
// ============================================================================

test "C API: arena lifecycle" {
    const path = "/tmp/test_c_api_arena.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);
}

test "C API: allocation" {
    const path = "/tmp/test_c_api_alloc.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    var ref: u64 = 0;
    const result = owl_arena_alloc(arena, 128, &ref);
    try std.testing.expectEqual(OWL_OK, result);
    try std.testing.expect(ref != 0);
}

test "C API: write and read" {
    const path = "/tmp/test_c_api_rw.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    // Create an object
    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{ 1, 2, 3, 4 };
    var obj = try Object.init(gpa, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(gpa);
    defer gpa.free(data);

    // Allocate and write
    var ref: u64 = 0;
    _ = owl_arena_alloc(arena, @intCast(data.len), &ref);
    const write_result = owl_arena_write(arena, ref, data.ptr, data.len);
    try std.testing.expectEqual(OWL_OK, write_result);

    // Read back
    var buffer: [1024]u8 = undefined;
    var bytes_read: usize = 0;
    const read_result = owl_arena_read(arena, ref, &buffer, buffer.len, &bytes_read);
    try std.testing.expectEqual(OWL_OK, read_result);
    try std.testing.expect(bytes_read > 0);
}

test "C API: delete" {
    const path = "/tmp/test_c_api_delete.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    // Create and write object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(gpa, 1, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(gpa);
    defer gpa.free(data);

    var ref: u64 = 0;
    _ = owl_arena_alloc(arena, @intCast(data.len), &ref);
    _ = owl_arena_write(arena, ref, data.ptr, data.len);

    // Delete
    const delete_result = owl_arena_delete(arena, ref);
    try std.testing.expectEqual(OWL_OK, delete_result);

    // Try to read - should fail
    var buffer: [1024]u8 = undefined;
    var bytes_read: usize = 0;
    const read_result = owl_arena_read(arena, ref, &buffer, buffer.len, &bytes_read);
    try std.testing.expectEqual(OWL_ERR_OBJECT_DELETED, read_result);
}

test "C API: stats" {
    const path = "/tmp/test_c_api_stats.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    var total: usize = 0;
    var used: usize = 0;
    var free: usize = 0;
    var count: u64 = 0;

    const result = owl_arena_stats(arena, &total, &used, &free, &count);
    try std.testing.expectEqual(OWL_OK, result);
    try std.testing.expectEqual(@as(usize, 1024 * 1024), total);
    try std.testing.expectEqual(@as(u64, 0), count);
}

test "C API: co-location" {
    const path = "/tmp/test_c_api_coloc.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    var ref1: u64 = 0;
    _ = owl_arena_alloc(arena, 100, &ref1);

    var ref2: u64 = 0;
    const result = owl_arena_alloc_near(arena, 100, ref1, &ref2);
    try std.testing.expectEqual(OWL_OK, result);

    // Check metrics
    var total: usize = 0;
    var successful: usize = 0;
    var failed: usize = 0;
    var rate: f64 = 0;

    _ = owl_arena_colocation_metrics(arena, &total, &successful, &failed, &rate);
    try std.testing.expect(total > 0);
}

test "C API: error handling" {
    // Null arena
    var ref: u64 = 0;
    const result = owl_arena_alloc(null, 100, &ref);
    try std.testing.expectEqual(OWL_ERR_INVALID_ARENA, result);

    // Invalid reference
    const path = "/tmp/test_c_api_err.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    defer owl_arena_destroy(arena);

    const write_result = owl_arena_write(arena, 0, null, 0);
    try std.testing.expectEqual(OWL_ERR_INVALID_REF, write_result);
}
