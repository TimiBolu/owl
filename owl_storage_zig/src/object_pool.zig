// Task 9.7: Object Pooling for Frequently Allocated Types
// Implements per-type object pools to reduce allocation overhead

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;

/// Maximum number of object types that can have dedicated pools
pub const MAX_POOLED_TYPES: usize = 16;

/// Maximum number of objects per pool
pub const MAX_POOL_SIZE: usize = 1024;

/// Statistics for a single object pool
pub const PoolStats = struct {
    /// Type tag for this pool
    type_tag: u16,
    /// Number of objects currently in the pool
    available: usize,
    /// Total allocations from this pool
    total_allocations: usize,
    /// Total returns to this pool
    total_returns: usize,
    /// Number of times pool was empty (had to allocate new)
    pool_misses: usize,
    /// Number of times pool was full (had to discard returned object)
    pool_overflows: usize,
};

/// A pool for a specific object type
const TypePool = struct {
    /// Type tag for objects in this pool
    type_tag: u16,
    /// Stack of available object references
    objects: std.ArrayList(ObjRef),
    /// Statistics
    stats: PoolStats,
    /// Mutex for thread safety
    mutex: std.Thread.Mutex,

    fn init(_: std.mem.Allocator, type_tag: u16) TypePool {
        return .{
            .type_tag = type_tag,
            .objects = std.ArrayList(ObjRef){},
            .stats = .{
                .type_tag = type_tag,
                .available = 0,
                .total_allocations = 0,
                .total_returns = 0,
                .pool_misses = 0,
                .pool_overflows = 0,
            },
            .mutex = std.Thread.Mutex{},
        };
    }

    fn deinit(self: *TypePool, allocator: std.mem.Allocator) void {
        self.objects.deinit(allocator);
    }

    /// Try to get an object from the pool
    fn acquire(self: *TypePool) ?ObjRef {
        self.mutex.lock();
        defer self.mutex.unlock();

        self.stats.total_allocations += 1;

        if (self.objects.items.len > 0) {
            const objref = self.objects.pop();
            self.stats.available = self.objects.items.len;
            return objref;
        }

        self.stats.pool_misses += 1;
        return null;
    }

    /// Return an object to the pool
    fn release(self: *TypePool, allocator: std.mem.Allocator, objref: ObjRef) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        self.stats.total_returns += 1;

        if (self.objects.items.len >= MAX_POOL_SIZE) {
            self.stats.pool_overflows += 1;
            return; // Pool is full, discard the object
        }

        try self.objects.append(allocator, objref);
        self.stats.available = self.objects.items.len;
    }

    /// Get current statistics
    fn getStats(self: *const TypePool) PoolStats {
        return self.stats;
    }
};

/// Object pool manager for frequently allocated types
pub const ObjectPool = struct {
    /// Array of type-specific pools
    pools: [MAX_POOLED_TYPES]?TypePool,
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    /// Number of active pools
    active_pools: usize,

    /// Initialize the object pool manager
    pub fn init(allocator: std.mem.Allocator) ObjectPool {
        return .{
            .pools = [_]?TypePool{null} ** MAX_POOLED_TYPES,
            .allocator = allocator,
            .active_pools = 0,
        };
    }

    /// Clean up all pools
    pub fn deinit(self: *ObjectPool) void {
        for (&self.pools) |*pool_opt| {
            if (pool_opt.*) |*pool| {
                pool.deinit(self.allocator);
            }
        }
    }

    /// Register a type for pooling
    pub fn registerType(self: *ObjectPool, type_tag: u16) !void {
        // Check if type is already registered
        for (self.pools) |pool_opt| {
            if (pool_opt) |pool| {
                if (pool.type_tag == type_tag) {
                    return; // Already registered
                }
            }
        }

        // Find an empty slot
        for (&self.pools) |*pool_opt| {
            if (pool_opt.* == null) {
                pool_opt.* = TypePool.init(self.allocator, type_tag);
                self.active_pools += 1;
                return;
            }
        }

        return error.TooManyPooledTypes;
    }

    /// Try to acquire an object from the pool for the given type
    pub fn acquire(self: *ObjectPool, type_tag: u16) ?ObjRef {
        for (&self.pools) |*pool_opt| {
            if (pool_opt.*) |*pool| {
                if (pool.type_tag == type_tag) {
                    return pool.acquire();
                }
            }
        }
        return null; // Type not pooled
    }

    /// Return an object to the pool
    pub fn release(self: *ObjectPool, type_tag: u16, objref: ObjRef) !void {
        for (&self.pools) |*pool_opt| {
            if (pool_opt.*) |*pool| {
                if (pool.type_tag == type_tag) {
                    try pool.release(self.allocator, objref);
                    return;
                }
            }
        }
        // Type not pooled, just ignore
    }

    /// Get statistics for a specific type
    pub fn getStats(self: *const ObjectPool, type_tag: u16) ?PoolStats {
        for (self.pools) |pool_opt| {
            if (pool_opt) |pool| {
                if (pool.type_tag == type_tag) {
                    return pool.getStats();
                }
            }
        }
        return null;
    }

    /// Get statistics for all pools
    pub fn getAllStats(self: *const ObjectPool, allocator: std.mem.Allocator) ![]PoolStats {
        var stats = std.ArrayList(PoolStats){};
        try stats.ensureTotalCapacity(allocator, self.active_pools);
        errdefer stats.deinit(allocator);

        for (self.pools) |pool_opt| {
            if (pool_opt) |pool| {
                try stats.append(allocator, pool.getStats());
            }
        }

        return stats.toOwnedSlice(allocator);
    }

    /// Calculate hit rate for a specific type
    pub fn hitRate(self: *const ObjectPool, type_tag: u16) f64 {
        if (self.getStats(type_tag)) |stats| {
            if (stats.total_allocations == 0) return 0.0;
            const hits = stats.total_allocations - stats.pool_misses;
            return (@as(f64, @floatFromInt(hits)) / @as(f64, @floatFromInt(stats.total_allocations))) * 100.0;
        }
        return 0.0;
    }
};

// ============================================================================
// Tests
// ============================================================================

test "ObjectPool: init and deinit" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try std.testing.expectEqual(@as(usize, 0), pool.active_pools);
}

test "ObjectPool: register type" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1); // User type
    try std.testing.expectEqual(@as(usize, 1), pool.active_pools);

    try pool.registerType(2); // Order type
    try std.testing.expectEqual(@as(usize, 2), pool.active_pools);

    // Registering same type again should be no-op
    try pool.registerType(1);
    try std.testing.expectEqual(@as(usize, 2), pool.active_pools);
}

test "ObjectPool: acquire and release" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1);

    // First acquire should miss (pool is empty)
    const ref1 = pool.acquire(1);
    try std.testing.expect(ref1 == null);

    // Release an object
    const objref = ObjRef.init(1000);
    try pool.release(1, objref);

    // Now acquire should succeed
    const ref2 = pool.acquire(1);
    try std.testing.expect(ref2 != null);
    try std.testing.expectEqual(objref.getOffset(), ref2.?.getOffset());

    // Next acquire should miss again
    const ref3 = pool.acquire(1);
    try std.testing.expect(ref3 == null);
}

test "ObjectPool: multiple objects" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1);

    // Release multiple objects
    try pool.release(1, ObjRef.init(1000));
    try pool.release(1, ObjRef.init(2000));
    try pool.release(1, ObjRef.init(3000));

    // Acquire them back (LIFO order)
    const ref1 = pool.acquire(1);
    try std.testing.expect(ref1 != null);
    try std.testing.expectEqual(@as(u64, 3000), ref1.?.getOffset());

    const ref2 = pool.acquire(1);
    try std.testing.expect(ref2 != null);
    try std.testing.expectEqual(@as(u64, 2000), ref2.?.getOffset());

    const ref3 = pool.acquire(1);
    try std.testing.expect(ref3 != null);
    try std.testing.expectEqual(@as(u64, 1000), ref3.?.getOffset());

    const ref4 = pool.acquire(1);
    try std.testing.expect(ref4 == null);
}

test "ObjectPool: statistics" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1);

    // Initial stats
    const stats1 = pool.getStats(1).?;
    try std.testing.expectEqual(@as(usize, 0), stats1.total_allocations);
    try std.testing.expectEqual(@as(usize, 0), stats1.total_returns);
    try std.testing.expectEqual(@as(usize, 0), stats1.pool_misses);

    // Acquire (miss)
    _ = pool.acquire(1);
    const stats2 = pool.getStats(1).?;
    try std.testing.expectEqual(@as(usize, 1), stats2.total_allocations);
    try std.testing.expectEqual(@as(usize, 1), stats2.pool_misses);

    // Release
    try pool.release(1, ObjRef.init(1000));
    const stats3 = pool.getStats(1).?;
    try std.testing.expectEqual(@as(usize, 1), stats3.total_returns);
    try std.testing.expectEqual(@as(usize, 1), stats3.available);

    // Acquire (hit)
    _ = pool.acquire(1);
    const stats4 = pool.getStats(1).?;
    try std.testing.expectEqual(@as(usize, 2), stats4.total_allocations);
    try std.testing.expectEqual(@as(usize, 1), stats4.pool_misses);
    try std.testing.expectEqual(@as(usize, 0), stats4.available);
}

test "ObjectPool: hit rate" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1);

    // All misses initially
    _ = pool.acquire(1);
    _ = pool.acquire(1);
    try std.testing.expectEqual(@as(f64, 0.0), pool.hitRate(1));

    // Add some objects
    try pool.release(1, ObjRef.init(1000));
    try pool.release(1, ObjRef.init(2000));

    // Now we should get hits
    _ = pool.acquire(1);
    _ = pool.acquire(1);

    // Hit rate should be 50% (2 hits out of 4 total)
    const rate = pool.hitRate(1);
    try std.testing.expect(rate > 49.0 and rate < 51.0);
}

test "ObjectPool: pool overflow" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1);

    // Fill the pool to capacity
    var i: usize = 0;
    while (i < MAX_POOL_SIZE) : (i += 1) {
        try pool.release(1, ObjRef.init(i * 100));
    }

    const stats1 = pool.getStats(1).?;
    try std.testing.expectEqual(MAX_POOL_SIZE, stats1.available);
    try std.testing.expectEqual(@as(usize, 0), stats1.pool_overflows);

    // Try to add one more (should overflow)
    try pool.release(1, ObjRef.init(999999));

    const stats2 = pool.getStats(1).?;
    try std.testing.expectEqual(MAX_POOL_SIZE, stats2.available);
    try std.testing.expectEqual(@as(usize, 1), stats2.pool_overflows);
}

test "ObjectPool: multiple types" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1); // User
    try pool.registerType(2); // Order
    try pool.registerType(3); // Product

    // Release objects of different types
    try pool.release(1, ObjRef.init(1000));
    try pool.release(2, ObjRef.init(2000));
    try pool.release(3, ObjRef.init(3000));

    // Acquire should get correct types
    const ref1 = pool.acquire(1);
    try std.testing.expect(ref1 != null);
    try std.testing.expectEqual(@as(u64, 1000), ref1.?.getOffset());

    const ref2 = pool.acquire(2);
    try std.testing.expect(ref2 != null);
    try std.testing.expectEqual(@as(u64, 2000), ref2.?.getOffset());

    const ref3 = pool.acquire(3);
    try std.testing.expect(ref3 != null);
    try std.testing.expectEqual(@as(u64, 3000), ref3.?.getOffset());
}

test "ObjectPool: unregistered type" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1);

    // Try to acquire unregistered type
    const ref = pool.acquire(99);
    try std.testing.expect(ref == null);

    // Try to release unregistered type (should not error)
    try pool.release(99, ObjRef.init(1000));
}

test "ObjectPool: getAllStats" {
    const allocator = std.testing.allocator;
    var pool = ObjectPool.init(allocator);
    defer pool.deinit();

    try pool.registerType(1);
    try pool.registerType(2);
    try pool.registerType(3);

    // Do some operations
    _ = pool.acquire(1);
    try pool.release(2, ObjRef.init(1000));
    _ = pool.acquire(3);

    const all_stats = try pool.getAllStats(allocator);
    defer allocator.free(all_stats);

    try std.testing.expectEqual(@as(usize, 3), all_stats.len);

    // Verify each type has stats
    var found_types: u8 = 0;
    for (all_stats) |stats| {
        if (stats.type_tag == 1 or stats.type_tag == 2 or stats.type_tag == 3) {
            found_types += 1;
        }
    }
    try std.testing.expectEqual(@as(u8, 3), found_types);
}
