// Lock-free page allocator for large objects
// Uses atomic operations and bitmap for page tracking

const std = @import("std");

pub const PAGE_SIZE: usize = 4096;

/// Lock-free page allocator using atomic bitmap
pub const PageAllocator = struct {
    /// Bitmap for page allocation (1 bit per page)
    /// 0 = free, 1 = allocated
    bitmap: []std.atomic.Value(u64),
    /// Number of pages
    num_pages: usize,
    /// Base pointer for the arena
    base_ptr: [*]u8,
    /// Arena size
    arena_size: usize,
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    /// Statistics
    stats: PageStats,
    /// Hint for next allocation (reduces search time)
    next_hint: std.atomic.Value(usize),

    pub fn init(allocator: std.mem.Allocator, base_ptr: [*]u8, arena_size: usize) !PageAllocator {
        const num_pages = (arena_size + PAGE_SIZE - 1) / PAGE_SIZE;
        const bitmap_size = (num_pages + 63) / 64; // Round up to u64 boundaries

        const bitmap = try allocator.alloc(std.atomic.Value(u64), bitmap_size);
        for (bitmap) |*entry| {
            entry.* = std.atomic.Value(u64).init(0);
        }

        return .{
            .bitmap = bitmap,
            .num_pages = num_pages,
            .base_ptr = base_ptr,
            .arena_size = arena_size,
            .allocator = allocator,
            .stats = PageStats.init(),
            .next_hint = std.atomic.Value(usize).init(0),
        };
    }

    pub fn deinit(self: *PageAllocator) void {
        self.allocator.free(self.bitmap);
    }

    /// Allocate contiguous pages (thread-safe)
    pub fn allocPages(self: *PageAllocator, num_pages: usize) !u32 {
        if (num_pages == 0) return error.InvalidSize;
        if (num_pages > self.num_pages) return error.OutOfMemory;

        _ = self.stats.alloc_attempts.fetchAdd(1, .monotonic);

        // Start search from hint
        const start_hint = self.next_hint.load(.monotonic);

        // Try to find contiguous free pages
        var search_start: usize = start_hint;
        var attempts: usize = 0;
        const max_attempts = self.num_pages * 2; // Prevent infinite loops

        while (attempts < max_attempts) : (attempts += 1) {
            if (self.tryAllocPagesAt(search_start, num_pages)) |page_id| {
                // Update hint for next allocation
                _ = self.next_hint.store((page_id + num_pages) % self.num_pages, .monotonic);
                _ = self.stats.successful_allocs.fetchAdd(1, .monotonic);
                _ = self.stats.pages_allocated.fetchAdd(num_pages, .monotonic);
                return @intCast(page_id * PAGE_SIZE);
            }

            // Move to next potential location
            search_start = (search_start + 1) % self.num_pages;
        }

        _ = self.stats.failed_allocs.fetchAdd(1, .monotonic);
        return error.OutOfMemory;
    }

    /// Try to allocate pages starting at a specific page ID
    fn tryAllocPagesAt(self: *PageAllocator, start_page: usize, num_pages: usize) ?usize {
        if (start_page + num_pages > self.num_pages) {
            return null;
        }

        // Check if all pages are free
        for (0..num_pages) |i| {
            const page_id = start_page + i;
            if (self.isPageAllocated(page_id)) {
                return null;
            }
        }

        // Try to atomically mark all pages as allocated
        for (0..num_pages) |i| {
            const page_id = start_page + i;
            if (!self.tryMarkPageAllocated(page_id)) {
                // Failed to allocate, rollback
                for (0..i) |j| {
                    self.markPageFree(start_page + j);
                }
                return null;
            }
        }

        return start_page;
    }

    /// Check if a page is allocated
    fn isPageAllocated(self: *const PageAllocator, page_id: usize) bool {
        const bitmap_idx = page_id / 64;
        const bit_idx: u6 = @intCast(page_id % 64);

        if (bitmap_idx >= self.bitmap.len) return true; // Out of bounds = allocated

        const bitmap_value = self.bitmap[bitmap_idx].load(.acquire);
        return (bitmap_value & (@as(u64, 1) << bit_idx)) != 0;
    }

    /// Try to atomically mark a page as allocated
    fn tryMarkPageAllocated(self: *PageAllocator, page_id: usize) bool {
        const bitmap_idx = page_id / 64;
        const bit_idx: u6 = @intCast(page_id % 64);

        if (bitmap_idx >= self.bitmap.len) return false;

        const bit_mask = @as(u64, 1) << bit_idx;

        while (true) {
            const current = self.bitmap[bitmap_idx].load(.acquire);

            // Check if already allocated
            if ((current & bit_mask) != 0) {
                return false;
            }

            const new_value = current | bit_mask;

            if (self.bitmap[bitmap_idx].cmpxchgWeak(
                current,
                new_value,
                .release,
                .acquire,
            )) |_| {
                // CAS failed, retry
                continue;
            } else {
                // CAS succeeded
                return true;
            }
        }
    }

    /// Mark a page as free
    fn markPageFree(self: *PageAllocator, page_id: usize) void {
        const bitmap_idx = page_id / 64;
        const bit_idx: u6 = @intCast(page_id % 64);

        if (bitmap_idx >= self.bitmap.len) return;

        const bit_mask = @as(u64, 1) << bit_idx;

        while (true) {
            const current = self.bitmap[bitmap_idx].load(.acquire);
            const new_value = current & ~bit_mask;

            if (self.bitmap[bitmap_idx].cmpxchgWeak(
                current,
                new_value,
                .release,
                .acquire,
            )) |_| {
                // CAS failed, retry
                continue;
            } else {
                // CAS succeeded
                return;
            }
        }
    }

    /// Free pages (thread-safe)
    pub fn freePages(self: *PageAllocator, offset: u32, num_pages: usize) void {
        const start_page = offset / PAGE_SIZE;

        for (0..num_pages) |i| {
            const page_id: usize = @intCast(start_page + i);
            self.markPageFree(page_id);
        }

        _ = self.stats.frees.fetchAdd(1, .monotonic);
        _ = self.stats.pages_freed.fetchAdd(num_pages, .monotonic);
    }

    /// Get statistics
    pub fn getStats(self: *const PageAllocator) PageStatsSnapshot {
        return .{
            .alloc_attempts = self.stats.alloc_attempts.load(.monotonic),
            .successful_allocs = self.stats.successful_allocs.load(.monotonic),
            .failed_allocs = self.stats.failed_allocs.load(.monotonic),
            .frees = self.stats.frees.load(.monotonic),
            .pages_allocated = self.stats.pages_allocated.load(.monotonic),
            .pages_freed = self.stats.pages_freed.load(.monotonic),
            .total_pages = self.num_pages,
        };
    }

    /// Get success rate
    pub fn successRate(self: *const PageAllocator) f64 {
        const stats = self.getStats();
        if (stats.alloc_attempts == 0) return 0.0;
        return (@as(f64, @floatFromInt(stats.successful_allocs)) / @as(f64, @floatFromInt(stats.alloc_attempts))) * 100.0;
    }

    /// Get current utilization
    pub fn utilization(self: *const PageAllocator) f64 {
        const stats = self.getStats();
        const pages_in_use = stats.pages_allocated - stats.pages_freed;
        return (@as(f64, @floatFromInt(pages_in_use)) / @as(f64, @floatFromInt(stats.total_pages))) * 100.0;
    }
};

/// Statistics for page allocator
const PageStats = struct {
    alloc_attempts: std.atomic.Value(usize),
    successful_allocs: std.atomic.Value(usize),
    failed_allocs: std.atomic.Value(usize),
    frees: std.atomic.Value(usize),
    pages_allocated: std.atomic.Value(usize),
    pages_freed: std.atomic.Value(usize),

    fn init() PageStats {
        return .{
            .alloc_attempts = std.atomic.Value(usize).init(0),
            .successful_allocs = std.atomic.Value(usize).init(0),
            .failed_allocs = std.atomic.Value(usize).init(0),
            .frees = std.atomic.Value(usize).init(0),
            .pages_allocated = std.atomic.Value(usize).init(0),
            .pages_freed = std.atomic.Value(usize).init(0),
        };
    }
};

/// Snapshot of page allocator statistics
pub const PageStatsSnapshot = struct {
    alloc_attempts: usize,
    successful_allocs: usize,
    failed_allocs: usize,
    frees: usize,
    pages_allocated: usize,
    pages_freed: usize,
    total_pages: usize,
};

// Tests
test "PageAllocator: basic allocation" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024; // 1MB = 256 pages
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var page_alloc = try PageAllocator.init(allocator, arena.ptr, arena_size);
    defer page_alloc.deinit();

    const offset1 = try page_alloc.allocPages(1);
    const offset2 = try page_alloc.allocPages(1);

    try std.testing.expect(offset1 != offset2);
    try std.testing.expectEqual(@as(u32, 0), offset1);
    try std.testing.expectEqual(@as(u32, PAGE_SIZE), offset2);
}

test "PageAllocator: contiguous allocation" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var page_alloc = try PageAllocator.init(allocator, arena.ptr, arena_size);
    defer page_alloc.deinit();

    const offset = try page_alloc.allocPages(4); // 4 contiguous pages
    try std.testing.expectEqual(@as(u32, 0), offset);

    const stats = page_alloc.getStats();
    try std.testing.expectEqual(@as(usize, 4), stats.pages_allocated);
}

test "PageAllocator: free and reuse" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var page_alloc = try PageAllocator.init(allocator, arena.ptr, arena_size);
    defer page_alloc.deinit();

    const offset1 = try page_alloc.allocPages(2);
    page_alloc.freePages(offset1, 2);

    // Reset hint to beginning to ensure reuse
    _ = page_alloc.next_hint.store(0, .monotonic);

    const offset2 = try page_alloc.allocPages(2);
    try std.testing.expectEqual(offset1, offset2); // Should reuse freed pages
}

test "PageAllocator: statistics" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var page_alloc = try PageAllocator.init(allocator, arena.ptr, arena_size);
    defer page_alloc.deinit();

    _ = try page_alloc.allocPages(1);
    _ = try page_alloc.allocPages(2);

    const stats = page_alloc.getStats();
    try std.testing.expectEqual(@as(usize, 2), stats.alloc_attempts);
    try std.testing.expectEqual(@as(usize, 2), stats.successful_allocs);
    try std.testing.expectEqual(@as(usize, 3), stats.pages_allocated);
}

test "PageAllocator: success rate" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024;
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var page_alloc = try PageAllocator.init(allocator, arena.ptr, arena_size);
    defer page_alloc.deinit();

    _ = try page_alloc.allocPages(1);
    _ = try page_alloc.allocPages(1);

    const success_rate = page_alloc.successRate();
    try std.testing.expectEqual(@as(f64, 100.0), success_rate);
}

test "PageAllocator: utilization" {
    const allocator = std.testing.allocator;
    const arena_size = 1024 * 1024; // 256 pages
    const arena = try allocator.alloc(u8, arena_size);
    defer allocator.free(arena);

    var page_alloc = try PageAllocator.init(allocator, arena.ptr, arena_size);
    defer page_alloc.deinit();

    _ = try page_alloc.allocPages(128); // Allocate half the pages

    const util = page_alloc.utilization();
    try std.testing.expect(util >= 49.0 and util <= 51.0); // ~50% utilization
}
