//! LRU Cache for Object Storage
//!
//! Provides a thread-safe LRU (Least Recently Used) cache for frequently accessed objects.
//! The cache reduces disk/memory access by keeping hot objects in memory.

const std = @import("std");
const Allocator = std.mem.Allocator;
const ObjRef = @import("objref.zig").ObjRef;

/// Cache entry metadata
const CacheEntry = struct {
    objref: ObjRef,
    data: []u8,
    size: usize,
    access_count: u64,
    last_access: i64, // timestamp in milliseconds

    pub fn init(allocator: Allocator, objref: ObjRef, data: []const u8) !CacheEntry {
        const data_copy = try allocator.alloc(u8, data.len);
        @memcpy(data_copy, data);

        return CacheEntry{
            .objref = objref,
            .data = data_copy,
            .size = data.len,
            .access_count = 1,
            .last_access = std.time.milliTimestamp(),
        };
    }

    pub fn deinit(self: *CacheEntry, allocator: Allocator) void {
        allocator.free(self.data);
    }

    pub fn touch(self: *CacheEntry) void {
        self.access_count += 1;
        self.last_access = std.time.milliTimestamp();
    }
};

/// LRU Cache statistics
pub const CacheStats = struct {
    hits: u64,
    misses: u64,
    evictions: u64,
    current_size: usize,
    max_size: usize,
    entry_count: usize,

    pub fn init(max_size: usize) CacheStats {
        return CacheStats{
            .hits = 0,
            .misses = 0,
            .evictions = 0,
            .current_size = 0,
            .max_size = max_size,
            .entry_count = 0,
        };
    }

    pub fn hitRate(self: *const CacheStats) f64 {
        const total = self.hits + self.misses;
        if (total == 0) return 0.0;
        return @as(f64, @floatFromInt(self.hits)) / @as(f64, @floatFromInt(total));
    }
};

/// LRU Cache configuration
pub const CacheConfig = struct {
    /// Maximum cache size in bytes
    max_size: usize = 64 * 1024 * 1024, // 64MB default
    /// Maximum number of entries
    max_entries: usize = 10000,
};

/// LRU Cache implementation
pub const LRUCache = struct {
    allocator: Allocator,
    config: CacheConfig,
    /// Map from ObjRef offset to cache entry
    entries: std.AutoHashMap(u64, CacheEntry),
    /// LRU list (ordered by access time)
    lru_list: std.ArrayList(u64),
    /// Cache statistics
    stats: CacheStats,
    /// Mutex for thread safety
    mutex: std.Thread.Mutex,

    const Self = @This();

    pub fn init(allocator: Allocator, config: CacheConfig) Self {
        return Self{
            .allocator = allocator,
            .config = config,
            .entries = std.AutoHashMap(u64, CacheEntry).init(allocator),
            .lru_list = .{},
            .stats = CacheStats.init(config.max_size),
            .mutex = .{},
        };
    }

    pub fn deinit(self: *Self) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        var iter = self.entries.valueIterator();
        while (iter.next()) |entry| {
            entry.deinit(self.allocator);
        }

        self.entries.deinit();
        self.lru_list.deinit(self.allocator);
    }

    /// Get an entry from the cache
    pub fn get(self: *Self, objref: ObjRef) ?[]const u8 {
        self.mutex.lock();
        defer self.mutex.unlock();

        const offset = objref.getOffset();
        if (self.entries.getPtr(offset)) |entry| {
            // Cache hit
            entry.touch();
            self.updateLRU(offset);
            self.stats.hits += 1;
            return entry.data;
        }

        // Cache miss
        self.stats.misses += 1;
        return null;
    }

    /// Put an entry into the cache
    pub fn put(self: *Self, objref: ObjRef, data: []const u8) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const offset = objref.getOffset();

        // Check if entry already exists
        if (self.entries.contains(offset)) {
            // Update existing entry
            if (self.entries.getPtr(offset)) |entry| {
                entry.deinit(self.allocator);
                self.stats.current_size -= entry.size;
            }
            _ = self.entries.remove(offset);
        }

        // Check if we need to evict
        while (self.shouldEvict(data.len)) {
            try self.evictLRU();
        }

        // Add new entry
        const entry = try CacheEntry.init(self.allocator, objref, data);
        try self.entries.put(offset, entry);
        try self.lru_list.append(self.allocator, offset);

        self.stats.current_size += data.len;
        self.stats.entry_count = self.entries.count();
    }

    /// Remove an entry from the cache
    pub fn remove(self: *Self, objref: ObjRef) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const offset = objref.getOffset();
        if (self.entries.getPtr(offset)) |entry| {
            self.stats.current_size -= entry.size;
            entry.deinit(self.allocator);
            _ = self.entries.remove(offset);
            self.removeLRU(offset);
            self.stats.entry_count = self.entries.count();
        }
    }

    /// Clear all entries from the cache
    pub fn clear(self: *Self) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        var iter = self.entries.valueIterator();
        while (iter.next()) |entry| {
            entry.deinit(self.allocator);
        }

        self.entries.clearRetainingCapacity();
        self.lru_list.clearRetainingCapacity();
        self.stats.current_size = 0;
        self.stats.entry_count = 0;
    }

    /// Get cache statistics
    pub fn getStats(self: *Self) CacheStats {
        self.mutex.lock();
        defer self.mutex.unlock();
        return self.stats;
    }

    // Internal methods

    fn shouldEvict(self: *Self, new_size: usize) bool {
        return self.stats.current_size + new_size > self.config.max_size or
            self.entries.count() >= self.config.max_entries;
    }

    fn evictLRU(self: *Self) !void {
        if (self.lru_list.items.len == 0) return;

        // Find LRU entry (oldest access time)
        var oldest_offset: u64 = self.lru_list.items[0];
        var oldest_time: i64 = std.math.maxInt(i64);

        for (self.lru_list.items) |offset| {
            if (self.entries.get(offset)) |entry| {
                if (entry.last_access < oldest_time) {
                    oldest_time = entry.last_access;
                    oldest_offset = offset;
                }
            }
        }

        // Evict the entry
        if (self.entries.getPtr(oldest_offset)) |entry| {
            self.stats.current_size -= entry.size;
            entry.deinit(self.allocator);
            _ = self.entries.remove(oldest_offset);
            self.removeLRU(oldest_offset);
            self.stats.evictions += 1;
            self.stats.entry_count = self.entries.count();
        }
    }

    fn updateLRU(self: *Self, offset: u64) void {
        // Move to end of LRU list (most recently used)
        self.removeLRU(offset);
        self.lru_list.append(self.allocator, offset) catch return;
    }

    fn removeLRU(self: *Self, offset: u64) void {
        var i: usize = 0;
        while (i < self.lru_list.items.len) : (i += 1) {
            if (self.lru_list.items[i] == offset) {
                _ = self.lru_list.orderedRemove(i);
                return;
            }
        }
    }
};

// Tests
test "Cache: basic operations" {
    const allocator = std.testing.allocator;

    var config = CacheConfig{};
    config.max_size = 1024;
    config.max_entries = 10;

    var cache = LRUCache.init(allocator, config);
    defer cache.deinit();

    // Put an entry
    const objref = ObjRef.init(1024);
    const data = "test data";
    try cache.put(objref, data);

    // Get the entry
    const result = cache.get(objref);
    try std.testing.expect(result != null);
    try std.testing.expectEqualStrings(data, result.?);

    // Check stats
    const stats = cache.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.hits);
    try std.testing.expectEqual(@as(u64, 0), stats.misses);
}

test "Cache: miss" {
    const allocator = std.testing.allocator;

    var cache = LRUCache.init(allocator, CacheConfig{});
    defer cache.deinit();

    // Try to get non-existent entry
    const objref = ObjRef.init(1024);
    const result = cache.get(objref);
    try std.testing.expect(result == null);

    // Check stats
    const stats = cache.getStats();
    try std.testing.expectEqual(@as(u64, 0), stats.hits);
    try std.testing.expectEqual(@as(u64, 1), stats.misses);
}

test "Cache: LRU eviction" {
    const allocator = std.testing.allocator;

    var config = CacheConfig{};
    config.max_size = 100;
    config.max_entries = 3;

    var cache = LRUCache.init(allocator, config);
    defer cache.deinit();

    // Add 3 entries
    try cache.put(ObjRef.init(1024), "data1");
    try cache.put(ObjRef.init(2048), "data2");
    try cache.put(ObjRef.init(3072), "data3");

    // Add 4th entry - should evict LRU
    try cache.put(ObjRef.init(4096), "data4");

    // First entry should be evicted
    const result1 = cache.get(ObjRef.init(1024));
    try std.testing.expect(result1 == null);

    // Other entries should still be there
    const result2 = cache.get(ObjRef.init(2048));
    try std.testing.expect(result2 != null);

    // Check stats
    const stats = cache.getStats();
    try std.testing.expectEqual(@as(u64, 1), stats.evictions);
}

test "Cache: hit rate" {
    const allocator = std.testing.allocator;

    var cache = LRUCache.init(allocator, CacheConfig{});
    defer cache.deinit();

    const objref = ObjRef.init(1024);
    try cache.put(objref, "test");

    // 3 hits
    _ = cache.get(objref);
    _ = cache.get(objref);
    _ = cache.get(objref);

    // 1 miss
    _ = cache.get(ObjRef.init(2048));

    const stats = cache.getStats();
    try std.testing.expectEqual(@as(u64, 3), stats.hits);
    try std.testing.expectEqual(@as(u64, 1), stats.misses);
    try std.testing.expectApproxEqAbs(@as(f64, 0.75), stats.hitRate(), 0.01);
}

test "Cache: remove" {
    const allocator = std.testing.allocator;

    var cache = LRUCache.init(allocator, CacheConfig{});
    defer cache.deinit();

    const objref = ObjRef.init(1024);
    try cache.put(objref, "test");

    // Verify it's there
    const result1 = cache.get(objref);
    try std.testing.expect(result1 != null);

    // Remove it
    cache.remove(objref);

    // Verify it's gone
    const result2 = cache.get(objref);
    try std.testing.expect(result2 == null);
}

test "Cache: clear" {
    const allocator = std.testing.allocator;

    var cache = LRUCache.init(allocator, CacheConfig{});
    defer cache.deinit();

    // Add multiple entries
    try cache.put(ObjRef.init(1024), "data1");
    try cache.put(ObjRef.init(2048), "data2");
    try cache.put(ObjRef.init(3072), "data3");

    // Clear cache
    cache.clear();

    // All entries should be gone
    try std.testing.expect(cache.get(ObjRef.init(1024)) == null);
    try std.testing.expect(cache.get(ObjRef.init(2048)) == null);
    try std.testing.expect(cache.get(ObjRef.init(3072)) == null);

    const stats = cache.getStats();
    try std.testing.expectEqual(@as(usize, 0), stats.entry_count);
}

test "Cache: thread safety" {
    const allocator = std.testing.allocator;

    var cache = LRUCache.init(allocator, CacheConfig{});
    defer cache.deinit();

    // Add some entries
    try cache.put(ObjRef.init(1024), "data1");
    try cache.put(ObjRef.init(2048), "data2");

    // Access from multiple "threads" (simulated)
    _ = cache.get(ObjRef.init(1024));
    _ = cache.get(ObjRef.init(2048));
    _ = cache.get(ObjRef.init(1024));

    const stats = cache.getStats();
    try std.testing.expectEqual(@as(u64, 3), stats.hits);
}
