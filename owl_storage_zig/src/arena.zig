// Task 1.4: Arena Allocator - Basic
// Memory-mapped arena with bump allocation

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;

/// Page size for arena allocation (4KB)
pub const PAGE_SIZE: usize = 4096;

/// Slot size for free list management (64 bytes)
pub const SLOT_SIZE: usize = 64;

/// Initial offset for bump allocator (skip offset 0 which is reserved for null)
pub const INITIAL_OFFSET: u32 = 64;

/// Co-location metrics for measuring effectiveness
pub const CoLocationMetrics = struct {
    /// Number of successful co-locations (allocated in same page)
    successful_colocations: std.atomic.Value(usize),
    /// Number of failed co-locations (had to allocate elsewhere)
    failed_colocations: std.atomic.Value(usize),
    /// Total co-location attempts
    total_attempts: std.atomic.Value(usize),

    pub fn init() CoLocationMetrics {
        return .{
            .successful_colocations = std.atomic.Value(usize).init(0),
            .failed_colocations = std.atomic.Value(usize).init(0),
            .total_attempts = std.atomic.Value(usize).init(0),
        };
    }

    /// Calculate co-location success rate as a percentage
    pub fn successRate(self: *const CoLocationMetrics) f64 {
        const total = self.total_attempts.load(.seq_cst);
        if (total == 0) return 0.0;

        const successful = self.successful_colocations.load(.seq_cst);
        return (@as(f64, @floatFromInt(successful)) / @as(f64, @floatFromInt(total))) * 100.0;
    }
};

/// Arena structure for memory-mapped storage.
/// Uses bump allocation with free list for reclaiming deleted slots.
pub const Arena = struct {
    /// Memory-mapped region
    mmap: []align(std.heap.page_size_min) u8,
    /// Free list of available slot IDs
    free_list: std.ArrayList(u32),
    /// Bump allocator pointer (offset in bytes)
    bump: std.atomic.Value(u32),
    /// Total arena size in bytes
    size: usize,
    /// Bytes currently used
    used: std.atomic.Value(usize),
    /// Number of objects allocated
    object_count: std.atomic.Value(u64),
    /// Page allocation tracking (page_id -> bytes used in page)
    page_usage: []std.atomic.Value(u32),
    /// Co-location metrics
    colocation_metrics: CoLocationMetrics,
    /// Allocator for internal structures
    allocator: std.mem.Allocator,

    /// Create a new arena with file-backed memory mapping
    pub fn init(allocator: std.mem.Allocator, path: []const u8, size: usize) !Arena {
        // Open or create the file
        const file = try std.fs.cwd().createFile(path, .{
            .read = true,
            .truncate = false,
        });
        defer file.close();

        // Set file size
        try file.setEndPos(size);

        // Memory map the file
        const mmap = try std.posix.mmap(
            null,
            size,
            std.posix.PROT.READ | std.posix.PROT.WRITE,
            .{ .TYPE = .SHARED },
            file.handle,
            0,
        );

        // Advise kernel about access pattern (random access)
        try std.posix.madvise(mmap.ptr, size, std.posix.MADV.RANDOM);

        // Calculate number of pages and allocate page tracking
        const num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
        const page_usage = try allocator.alloc(std.atomic.Value(u32), num_pages);
        for (page_usage) |*p| {
            p.* = std.atomic.Value(u32).init(0);
        }

        return .{
            .mmap = mmap,
            .free_list = std.ArrayList(u32){},
            .bump = std.atomic.Value(u32).init(INITIAL_OFFSET),
            .size = size,
            .used = std.atomic.Value(usize).init(0),
            .object_count = std.atomic.Value(u64).init(0),
            .page_usage = page_usage,
            .colocation_metrics = CoLocationMetrics.init(),
            .allocator = allocator,
        };
    }

    /// Clean up arena resources
    pub fn deinit(self: *Arena) void {
        std.posix.munmap(self.mmap);
        self.free_list.deinit(self.allocator);
        self.allocator.free(self.page_usage);
    }

    /// Allocate space for an object
    pub fn alloc(self: *Arena, size: u32) !ObjRef {
        return self.allocNear(size, null);
    }

    /// Allocate space near another object for co-location
    pub fn allocNear(self: *Arena, size: u32, near: ?ObjRef) !ObjRef {
        // Try free list first for small allocations
        if (size <= SLOT_SIZE) {
            if (self.free_list.pop()) |slot| {
                _ = self.object_count.fetchAdd(1, .seq_cst);
                return ObjRef.init(@as(u64, slot) * @as(u64, SLOT_SIZE));
            }
        }

        // If near reference provided, try co-location
        const near_page_id = if (near) |near_ref| blk: {
            _ = self.colocation_metrics.total_attempts.fetchAdd(1, .seq_cst);

            const page_id = near_ref.pageId();

            // Try to allocate in same page
            if (self.allocInPage(page_id, size)) |offset| {
                _ = self.object_count.fetchAdd(1, .seq_cst);
                _ = self.colocation_metrics.successful_colocations.fetchAdd(1, .seq_cst);
                return ObjRef.init(offset);
            }

            // Try adjacent pages
            if (page_id > 0) {
                if (self.allocInPage(page_id - 1, size)) |offset| {
                    _ = self.object_count.fetchAdd(1, .seq_cst);
                    _ = self.colocation_metrics.successful_colocations.fetchAdd(1, .seq_cst);
                    return ObjRef.init(offset);
                }
            }
            if (self.allocInPage(page_id + 1, size)) |offset| {
                _ = self.object_count.fetchAdd(1, .seq_cst);
                _ = self.colocation_metrics.successful_colocations.fetchAdd(1, .seq_cst);
                return ObjRef.init(offset);
            }

            // Co-location in specific pages failed, will fall back to bump allocation
            break :blk page_id;
        } else null;

        // Fall back to bump allocation
        const offset = self.bump.fetchAdd(size, .seq_cst);
        if (offset + size > self.size) {
            return error.OutOfMemory;
        }

        _ = self.used.fetchAdd(size, .seq_cst);
        _ = self.object_count.fetchAdd(1, .seq_cst);

        // Update page usage
        const result_page_id: u32 = @intCast(offset / PAGE_SIZE);
        const page_idx: usize = @intCast(result_page_id);
        if (page_idx < self.page_usage.len) {
            _ = self.page_usage[page_idx].fetchAdd(size, .seq_cst);
        }

        // Check if bump allocation ended up co-located anyway
        if (near_page_id) |target_page| {
            const page_diff = if (result_page_id > target_page)
                result_page_id - target_page
            else
                target_page - result_page_id;

            if (page_diff <= 1) {
                // Success! Bump allocation put us on same or adjacent page
                _ = self.colocation_metrics.successful_colocations.fetchAdd(1, .seq_cst);
            } else {
                // Failed - ended up too far away
                _ = self.colocation_metrics.failed_colocations.fetchAdd(1, .seq_cst);
            }
        }

        return ObjRef.init(offset);
    }

    /// Try to allocate in a specific page
    fn allocInPage(self: *Arena, page_id: u32, size: u32) ?u64 {
        const page_idx: usize = @intCast(page_id);
        if (page_idx >= self.page_usage.len) return null;

        const page_start = @as(u64, page_id) * PAGE_SIZE;

        // Atomically reserve space in the page
        // This prevents race conditions by doing fetch-and-add BEFORE checking
        const old_usage = self.page_usage[page_idx].fetchAdd(size, .seq_cst);

        // Check if the allocation fits in the page
        if (old_usage + size <= PAGE_SIZE) {
            // Success! We reserved space and it fits
            const offset = page_start + old_usage;

            _ = self.used.fetchAdd(size, .seq_cst);

            // Update bump pointer if needed
            const current_bump = self.bump.load(.seq_cst);
            const new_bump: u32 = @intCast(@max(current_bump, offset + size));
            _ = self.bump.cmpxchgWeak(current_bump, new_bump, .seq_cst, .seq_cst);

            return offset;
        }

        // Allocation doesn't fit - rollback the page usage
        _ = self.page_usage[page_idx].fetchSub(size, .seq_cst);
        return null;
    }

    /// Free an object slot for reuse
    pub fn free(self: *Arena, objref: ObjRef) !void {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        // Add to free list (only for slot-sized allocations)
        const slot_id: u32 = @intCast(objref.getOffset() / SLOT_SIZE);
        try self.free_list.append(self.allocator, slot_id);
        _ = self.object_count.fetchSub(1, .seq_cst);
    }

    /// Get the base pointer for swizzling
    pub fn basePtr(self: *const Arena) [*]u8 {
        return self.mmap.ptr;
    }

    /// Write data to a specific offset
    pub fn writeAt(self: *Arena, offset: u64, data: []const u8) !void {
        const start: usize = @intCast(offset);
        const end = start + data.len;

        if (end > self.size) {
            return error.OutOfBounds;
        }

        @memcpy(self.mmap[start..end], data);
    }

    /// Read data from a specific offset
    pub fn readAt(self: *const Arena, offset: u64, len: usize) ![]const u8 {
        const start: usize = @intCast(offset);
        const end = start + len;

        if (end > self.size) {
            return error.OutOfBounds;
        }

        return self.mmap[start..end];
    }

    /// Write an object to the arena
    pub fn write(self: *Arena, objref: ObjRef, data: []const u8) !void {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        // Validate data contains a valid object header
        if (data.len < ObjectHeader.headerSize()) {
            return error.DataTooSmall;
        }

        // Bounds checking
        const offset = objref.getOffset();
        if (offset >= self.size or offset + data.len > self.size) {
            return error.OutOfBounds;
        }

        // Write data
        try self.writeAt(offset, data);
    }

    /// Read an object from the arena
    pub fn read(self: *const Arena, objref: ObjRef, allocator: std.mem.Allocator) !Object {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        const offset = objref.getOffset();
        if (offset >= self.size) {
            return error.OutOfBounds;
        }

        // Read header first to get total size
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        const header = try ObjectHeader.fromBytes(header_bytes);

        // Validate header
        if (header.isDeleted()) {
            return error.ObjectDeleted;
        }

        // Bounds check for full object
        if (offset + header.size > self.size) {
            return error.OutOfBounds;
        }

        // Read full object
        const full_bytes = try self.readAt(offset, header.size);

        // Deserialize object
        return try Object.fromBytes(allocator, full_bytes);
    }

    /// Delete an object (mark as deleted and add to free list)
    pub fn delete(self: *Arena, objref: ObjRef) !void {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        const offset = objref.getOffset();
        if (offset >= self.size) {
            return error.OutOfBounds;
        }

        // Read header
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        var header = try ObjectHeader.fromBytes(header_bytes);

        // Mark as deleted
        header.setDeleted(true);

        // Write back modified header
        const modified_header = header.toBytes();
        try self.writeAt(offset, &modified_header);

        // Add to free list
        try self.free(objref);
    }

    /// Get statistics about the arena
    pub fn stats(self: *const Arena) ArenaStats {
        return .{
            .total_size = self.size,
            .used_size = self.used.load(.seq_cst),
            .free_size = self.size - self.used.load(.seq_cst),
            .object_count = self.object_count.load(.seq_cst),
            .free_slots = self.free_list.items.len,
        };
    }

    /// Get co-location metrics
    pub fn colocationMetrics(self: *const Arena) *const CoLocationMetrics {
        return &self.colocation_metrics;
    }

    /// Get page utilization for a specific page
    pub fn pageUtilization(self: *const Arena, page_id: usize) f64 {
        if (page_id >= self.page_usage.len) return 0.0;

        const used = self.page_usage[page_id].load(.seq_cst);
        return (@as(f64, @floatFromInt(used)) / @as(f64, @floatFromInt(PAGE_SIZE))) * 100.0;
    }
};

/// Arena statistics
pub const ArenaStats = struct {
    total_size: usize,
    used_size: usize,
    free_size: usize,
    object_count: u64,
    free_slots: usize,
};

// ============================================================================
// Tests
// ============================================================================

fn createTestArena(allocator: std.mem.Allocator) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 1024 * 1024); // 1MB
    return .{ .arena = arena, .dir = tmp };
}

test "Arena: creation and cleanup" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    try std.testing.expectEqual(@as(usize, 1024 * 1024), test_arena.arena.size);
    try std.testing.expectEqual(@as(usize, 0), test_arena.arena.used.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 0), test_arena.arena.object_count.load(.seq_cst));
}

test "Arena: basic allocation" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const ref1 = try test_arena.arena.alloc(100);
    try std.testing.expect(!ref1.isNull());
    try std.testing.expectEqual(@as(u64, INITIAL_OFFSET), ref1.getOffset());

    const ref2 = try test_arena.arena.alloc(200);
    try std.testing.expectEqual(@as(u64, INITIAL_OFFSET + 100), ref2.getOffset());

    try std.testing.expectEqual(@as(usize, 300), test_arena.arena.used.load(.seq_cst));
    try std.testing.expectEqual(@as(u64, 2), test_arena.arena.object_count.load(.seq_cst));
}

test "Arena: free list reuse" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate a slot-sized object
    const ref = try test_arena.arena.alloc(64);
    try std.testing.expectEqual(@as(u64, 1), test_arena.arena.object_count.load(.seq_cst));

    // Free it
    try test_arena.arena.free(ref);
    try std.testing.expectEqual(@as(u64, 0), test_arena.arena.object_count.load(.seq_cst));
    try std.testing.expectEqual(@as(usize, 1), test_arena.arena.free_list.items.len);

    // Allocate again - should reuse the free slot
    const ref2 = try test_arena.arena.alloc(64);
    try std.testing.expectEqual(ref.getOffset(), ref2.getOffset());
    try std.testing.expectEqual(@as(usize, 0), test_arena.arena.free_list.items.len);
}

test "Arena: write and read" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const data = "Hello, Arena!";
    const ref = try test_arena.arena.alloc(@intCast(data.len));

    // Write data
    try test_arena.arena.writeAt(ref.getOffset(), data);

    // Read data back
    const read_data = try test_arena.arena.readAt(ref.getOffset(), data.len);
    try std.testing.expectEqualStrings(data, read_data);
}

test "Arena: out of memory" {
    const allocator = std.testing.allocator;
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();

    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "small_arena.dat" });
    defer allocator.free(file_path);

    var arena = try Arena.init(allocator, file_path, 1000);
    defer arena.deinit();

    // Allocate until we run out of space
    var count: usize = 0;
    while (count < 20) : (count += 1) {
        _ = arena.alloc(100) catch break;
    }

    // Next allocation should fail
    const result = arena.alloc(100);
    try std.testing.expectError(error.OutOfMemory, result);
}

test "Arena: object write and read" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Create an object
    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{ 1, 2, 3, 4 };
    var obj = try Object.init(allocator, 1, id, &fixed, "test", &[_]ObjRef{});
    defer obj.deinit();

    // Serialize
    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    // Allocate space and write
    const ref = try test_arena.arena.alloc(@intCast(data.len));
    try test_arena.arena.write(ref, data);

    // Read back
    var obj2 = try test_arena.arena.read(ref, allocator);
    defer obj2.deinit();

    try std.testing.expectEqual(obj.header.type_tag, obj2.header.type_tag);
    try std.testing.expectEqualSlices(u8, obj.fixed, obj2.fixed);
}

test "Arena: delete object" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Create and write object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const ref = try test_arena.arena.alloc(@intCast(data.len));
    try test_arena.arena.write(ref, data);

    // Delete
    try test_arena.arena.delete(ref);

    // Try to read - should fail
    const result = test_arena.arena.read(ref, allocator);
    try std.testing.expectError(error.ObjectDeleted, result);
}

test "Arena: stats" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const stats1 = test_arena.arena.stats();
    try std.testing.expectEqual(@as(usize, 0), stats1.used_size);
    try std.testing.expectEqual(@as(u64, 0), stats1.object_count);

    _ = try test_arena.arena.alloc(100);
    _ = try test_arena.arena.alloc(200);

    const stats2 = test_arena.arena.stats();
    try std.testing.expectEqual(@as(usize, 300), stats2.used_size);
    try std.testing.expectEqual(@as(u64, 2), stats2.object_count);
}

test "Arena: bounds checking" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Try to write beyond bounds
    const ref = ObjRef.init(test_arena.arena.size - 10);
    const data = [_]u8{0} ** 100;

    const result = test_arena.arena.writeAt(ref.getOffset(), &data);
    try std.testing.expectError(error.OutOfBounds, result);
}

test "Arena: null reference handling" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const null_ref = ObjRef.nullRef();

    // Write should fail
    const write_result = test_arena.arena.write(null_ref, &[_]u8{ 1, 2, 3 });
    try std.testing.expectError(error.InvalidReference, write_result);

    // Read should fail
    const read_result = test_arena.arena.read(null_ref, allocator);
    try std.testing.expectError(error.InvalidReference, read_result);

    // Delete should fail
    const delete_result = test_arena.arena.delete(null_ref);
    try std.testing.expectError(error.InvalidReference, delete_result);
}

test "Arena: co-location same page" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate first object
    const ref1 = try test_arena.arena.alloc(500);
    const page1 = ref1.pageId();

    // Allocate near the first object - should be in same page
    const ref2 = try test_arena.arena.allocNear(500, ref1);
    const page2 = ref2.pageId();

    try std.testing.expectEqual(page1, page2);

    // Check metrics
    const metrics = test_arena.arena.colocationMetrics();
    try std.testing.expectEqual(@as(usize, 1), metrics.total_attempts.load(.seq_cst));
    try std.testing.expectEqual(@as(usize, 1), metrics.successful_colocations.load(.seq_cst));
    try std.testing.expectEqual(@as(f64, 100.0), metrics.successRate());
}

test "Arena: co-location adjacent page" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate a large object that fills most of a page
    const ref1 = try test_arena.arena.alloc(3500);
    const page1 = ref1.pageId();

    // Try to allocate near it - should go to adjacent page
    const ref2 = try test_arena.arena.allocNear(1000, ref1);
    const page2 = ref2.pageId();

    // Should be in same or adjacent page
    const page_diff = if (page2 > page1) page2 - page1 else page1 - page2;
    try std.testing.expect(page_diff <= 1);
}

test "Arena: co-location metrics" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate several objects with co-location
    const ref1 = try test_arena.arena.alloc(100);

    // These should succeed
    _ = try test_arena.arena.allocNear(100, ref1);
    _ = try test_arena.arena.allocNear(100, ref1);

    const metrics = test_arena.arena.colocationMetrics();
    try std.testing.expect(metrics.total_attempts.load(.seq_cst) > 0);
    try std.testing.expect(metrics.successRate() > 0.0);
}

test "Arena: page utilization" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const ref = try test_arena.arena.alloc(2048);
    const page_id: usize = @intCast(ref.pageId());

    const utilization = test_arena.arena.pageUtilization(page_id);
    try std.testing.expect(utilization >= 50.0); // At least 50% utilized
    try std.testing.expect(utilization <= 100.0);
}
