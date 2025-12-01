// Task 10.1: Decompression Cache
// Lock-free LRU cache for hot decompressed objects

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;

/// Cache entry for decompressed data
const CacheEntry = struct {
    objref: ObjRef,
    data: []u8,
    access_count: std.atomic.Value(u64),
    last_access: std.atomic.Value(i64),
    next: ?*CacheEntry,

    pub fn init(allocator: std.mem.Allocator, objref: ObjRef, data: []const u8) !*CacheEntry {
        const entry = try allocator.create(CacheEntry);
        const data_copy = try allocator.dupe(u8, data);

        entry.* = .{
            .objref = objref,
            .data = data_copy,
            .access_count = std.atomic.Value(u64).init(1),
            .last_access = std.atomic.Value(i64).init(std.time.timestamp()),
            .next = null,
        };

        return entry;
    }

    pub fn deinit(self: *CacheEntry, allocator: std.mem.Allocator) void {
        allocator.free(self.data);
        allocator.destroy(self);
    }

    pub fn touch(self: *CacheEntry) void {
        _ = self.access_count.fetchAdd(1, .seq_cst);
        self.last_access.store(std.time.timestamp(), .seq_cst);
    }
};

/// Lock-free LRU cache statistics
pub const CacheStats = struct {
    hits: std.atomic.Value(u64),
    misses: std.atomic.Value(u64),
    evictions: std.atomic.Value(u64),
    current_size: std.atomic.Value(usize),
    current_entries: std.atomic.Value(usize),

    pub fn init() CacheStats {
        return .{
            .hits = std.atomic.Value(u64).init(0),
            .misses = std.atomic.Value(u64).init(0),
            .evictions = std.atomic.Value(u64).init(0),
            .current_size = std.atomic.Value(usize).init(0),
            .current_entries = std.atomic.Value(usize).init(0),
        };
    }

    pub fn hitRate(self: *const CacheStats) f64 {
        const hits = self.hits.load(.seq_cst);
        const misses = self.misses.load(.seq_cst);
        const total = hits + misses;

        if (total == 0) return 0.0;

        return (@as(f64, @floatFromInt(hits)) / @as(f64, @floatFromInt(total))) * 100.0;
    }
};

/// Lock-free decompression cache using atomic operations
pub const DecompressionCache = struct {
    allocator: std.mem.Allocator,
    max_size: usize,
    max_entries: usize,
    stats: CacheStats,
    entries: std.atomic.Value(?*CacheEntry),
    mutex: std.Thread.Mutex, // For eviction only

    pub fn init(allocator: std.mem.Allocator, max_size: usize, max_entries: usize) DecompressionCache {
        return .{
            .allocator = allocator,
            .max_size = max_size,
            .max_entries = max_entries,
            .stats = CacheStats.init(),
            .entries = std.atomic.Value(?*CacheEntry).init(null),
            .mutex = std.Thread.Mutex{},
        };
    }

    pub fn deinit(self: *DecompressionCache) void {
        // Clear all entries
        var current = self.entries.load(.seq_cst);
        while (current) |entry| {
            const next = entry.next;
            entry.deinit(self.allocator);
            current = next;
        }
    }

    /// Get cached decompressed data (lock-free read)
    pub fn get(self: *DecompressionCache, objref: ObjRef) ?[]const u8 {
        var current = self.entries.load(.seq_cst);

        while (current) |entry| {
            if (entry.objref.getOffset() == objref.getOffset()) {
                // Cache hit
                entry.touch();
                _ = self.stats.hits.fetchAdd(1, .seq_cst);
                return entry.data;
            }
            current = entry.next;
        }

        // Cache miss
        _ = self.stats.misses.fetchAdd(1, .seq_cst);
        return null;
    }

    /// Put decompressed data in cache (uses mutex for insertion)
    pub fn put(self: *DecompressionCache, objref: ObjRef, data: []const u8) !void {
        // Check if already cached
        if (self.get(objref)) |_| {
            return; // Already cached
        }

        // Create new entry
        const new_entry = try CacheEntry.init(self.allocator, objref, data);
        errdefer new_entry.deinit(self.allocator);

        // Lock for insertion
        self.mutex.lock();
        defer self.mutex.unlock();

        // Check size limits and evict if necessary
        const current_size = self.stats.current_size.load(.seq_cst);
        const current_entries = self.stats.current_entries.load(.seq_cst);

        if (current_size + data.len > self.max_size or current_entries >= self.max_entries) {
            try self.evictLRU();
        }

        // Insert at head
        new_entry.next = self.entries.load(.seq_cst);
        self.entries.store(new_entry, .seq_cst);

        _ = self.stats.current_size.fetchAdd(data.len, .seq_cst);
        _ = self.stats.current_entries.fetchAdd(1, .seq_cst);
    }

    /// Evict least recently used entry (must hold mutex)
    fn evictLRU(self: *DecompressionCache) !void {
        var current = self.entries.load(.seq_cst);
        var prev: ?*CacheEntry = null;
        var lru_entry: ?*CacheEntry = null;
        var lru_prev: ?*CacheEntry = null;
        var oldest_access: i64 = std.math.maxInt(i64);

        // Find LRU entry
        while (current) |entry| {
            const last_access = entry.last_access.load(.seq_cst);
            if (last_access < oldest_access) {
                oldest_access = last_access;
                lru_entry = entry;
                lru_prev = prev;
            }
            prev = entry;
            current = entry.next;
        }

        // Remove LRU entry
        if (lru_entry) |entry| {
            if (lru_prev) |p| {
                p.next = entry.next;
            } else {
                // Removing head
                self.entries.store(entry.next, .seq_cst);
            }

            _ = self.stats.current_size.fetchSub(entry.data.len, .seq_cst);
            _ = self.stats.current_entries.fetchSub(1, .seq_cst);
            _ = self.stats.evictions.fetchAdd(1, .seq_cst);

            entry.deinit(self.allocator);
        }
    }

    /// Clear all cached entries
    pub fn clear(self: *DecompressionCache) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        var current = self.entries.load(.seq_cst);
        while (current) |entry| {
            const next = entry.next;
            _ = self.stats.current_size.fetchSub(entry.data.len, .seq_cst);
            _ = self.stats.current_entries.fetchSub(1, .seq_cst);
            entry.deinit(self.allocator);
            current = next;
        }

        self.entries.store(null, .seq_cst);
    }

    /// Get cache statistics
    pub fn getStats(self: *const DecompressionCache) *const CacheStats {
        return &self.stats;
    }
};

// ============================================================================
// Tests
// ============================================================================

test "DecompressionCache: basic operations" {
    const allocator = std.testing.allocator;
    var cache = DecompressionCache.init(allocator, 1024 * 1024, 100);
    defer cache.deinit();

    const objref = ObjRef.init(100);
    const data = "Hello, World!";

    // Initially not cached
    try std.testing.expect(cache.get(objref) == null);

    // Put in cache
    try cache.put(objref, data);

    // Now should be cached
    const cached = cache.get(objref);
    try std.testing.expect(cached != null);
    try std.testing.expectEqualStrings(data, cached.?);
}

test "DecompressionCache: hit rate" {
    const allocator = std.testing.allocator;
    var cache = DecompressionCache.init(allocator, 1024 * 1024, 100);
    defer cache.deinit();

    const objref1 = ObjRef.init(100);
    const objref2 = ObjRef.init(200);
    const data = "Test data";

    try cache.put(objref1, data);

    // Hit
    _ = cache.get(objref1);

    // Miss
    _ = cache.get(objref2);

    const stats = cache.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.hits.load(.seq_cst));
    // Note: put() calls get() internally to check if already cached, so we have 2 misses
    try std.testing.expectEqual(@as(u64, 2), stats.misses.load(.seq_cst));
    try std.testing.expectApproxEqAbs(@as(f64, 33.3), stats.hitRate(), 1.0);
}

test "DecompressionCache: eviction" {
    const allocator = std.testing.allocator;
    // Small cache that can only hold 2 entries
    var cache = DecompressionCache.init(allocator, 100, 2);
    defer cache.deinit();

    const objref1 = ObjRef.init(100);
    const objref2 = ObjRef.init(200);
    const objref3 = ObjRef.init(300);
    const data = "Test";

    try cache.put(objref1, data);
    try cache.put(objref2, data);

    // Access objref1 to make it more recently used
    _ = cache.get(objref1);

    // This should evict objref2 (LRU)
    try cache.put(objref3, data);

    // objref1 and objref3 should be cached
    try std.testing.expect(cache.get(objref1) != null);
    try std.testing.expect(cache.get(objref3) != null);

    // objref2 should be evicted
    try std.testing.expect(cache.get(objref2) == null);
}

test "DecompressionCache: clear" {
    const allocator = std.testing.allocator;
    var cache = DecompressionCache.init(allocator, 1024 * 1024, 100);
    defer cache.deinit();

    const objref1 = ObjRef.init(100);
    const objref2 = ObjRef.init(200);
    const data = "Test data";

    try cache.put(objref1, data);
    try cache.put(objref2, data);

    try std.testing.expect(cache.get(objref1) != null);
    try std.testing.expect(cache.get(objref2) != null);

    cache.clear();

    try std.testing.expect(cache.get(objref1) == null);
    try std.testing.expect(cache.get(objref2) == null);
}

test "DecompressionCache: size tracking" {
    const allocator = std.testing.allocator;
    var cache = DecompressionCache.init(allocator, 1024 * 1024, 100);
    defer cache.deinit();

    const objref = ObjRef.init(100);
    const data = "Hello, World!";

    try cache.put(objref, data);

    const stats = cache.getStats();
    try std.testing.expectEqual(@as(usize, 1), stats.current_entries.load(.seq_cst));
    try std.testing.expectEqual(@as(usize, data.len), stats.current_size.load(.seq_cst));
}
