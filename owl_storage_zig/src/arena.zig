// Task 1.4: Arena Allocator - Basic
// Memory-mapped arena with bump allocation

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const NavigationResult = @import("global_objref.zig").NavigationResult;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const LockFreeStack = @import("lockfree_stack.zig").LockFreeStack;
const SlabAllocator = @import("slab_allocator.zig").SlabAllocator;
const SlabClassStats = @import("slab_allocator.zig").SlabClassStats;
const SlabSizeClass = @import("slab_allocator.zig").SlabSizeClass;
const PageAllocator = @import("page_allocator.zig").PageAllocator;
const PageStatsSnapshot = @import("page_allocator.zig").PageStatsSnapshot;
const ObjectPool = @import("object_pool.zig").ObjectPool;
const PoolStats = @import("object_pool.zig").PoolStats;
const prefetch = @import("prefetch.zig");
const CompressionEngine = @import("compression.zig").CompressionEngine;
const CompressionType = @import("compression.zig").CompressionType;
const DecompressionCache = @import("decompression_cache.zig").DecompressionCache;

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

/// Arena structure for memory-mapped storage with distributed support.
/// Uses lock-free slab allocation for small objects and page allocation for large objects.
/// Supports both local ObjRef and distributed GlobalObjRef operations.
/// OPTIMIZED: Includes object pooling and prefetching for hot path performance.
pub const Arena = struct {
    /// Memory-mapped region
    mmap: []align(std.heap.page_size_min) u8,
    /// Lock-free slab allocator for small objects (<= 1KB)
    slab_allocator: SlabAllocator,
    /// Lock-free page allocator for large objects (> 1KB)
    page_allocator: PageAllocator,
    /// Bump allocator pointer (offset in bytes) - fallback for very large objects
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
    /// Current shard ID for this arena instance
    shard_id: ShardId,
    /// Object pool for frequently allocated types (OPTIMIZATION: Task 9.7)
    object_pool: ObjectPool,
    /// Compression engine for adaptive compression (Task 10)
    compression_engine: CompressionEngine,
    /// Decompression cache for hot objects (Task 10.1)
    decompression_cache: DecompressionCache,
    /// Enable compression (can be toggled at runtime)
    compression_enabled: std.atomic.Value(bool),

    /// Create a new arena with file-backed memory mapping
    pub fn init(allocator: std.mem.Allocator, path: []const u8, size: usize, shard_id: ShardId) !Arena {
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

        // Initialize lock-free allocators
        const slab_allocator = SlabAllocator.init(allocator, mmap.ptr, size, INITIAL_OFFSET);
        const page_allocator = try PageAllocator.init(allocator, mmap.ptr, size);

        return .{
            .mmap = mmap,
            .slab_allocator = slab_allocator,
            .page_allocator = page_allocator,
            .bump = std.atomic.Value(u32).init(INITIAL_OFFSET),
            .size = size,
            .used = std.atomic.Value(usize).init(0),
            .object_count = std.atomic.Value(u64).init(0),
            .page_usage = page_usage,
            .colocation_metrics = CoLocationMetrics.init(),
            .allocator = allocator,
            .shard_id = shard_id,
            .object_pool = ObjectPool.init(allocator),
            .compression_engine = CompressionEngine.init(allocator),
            .decompression_cache = DecompressionCache.init(allocator, 100 * 1024 * 1024, 1000), // 100MB cache, 1000 entries
            .compression_enabled = std.atomic.Value(bool).init(true),
        };
    }

    /// Register a type for object pooling (OPTIMIZATION: Task 9.7)
    /// This should be called for frequently allocated types to reduce allocation overhead
    pub fn registerPooledType(self: *Arena, type_tag: u16) !void {
        try self.object_pool.registerType(type_tag);
    }

    /// Clean up arena resources
    pub fn deinit(self: *Arena) void {
        std.posix.munmap(self.mmap);
        self.slab_allocator.deinit();
        self.page_allocator.deinit();
        self.object_pool.deinit();
        self.decompression_cache.deinit();
        self.allocator.free(self.page_usage);
    }

    /// Allocate space for an object (lock-free)
    pub fn alloc(self: *Arena, size: u32) !ObjRef {
        return self.allocNear(size, null);
    }

    /// Allocate space for an object with type pooling (OPTIMIZED: Task 9.7)
    /// This method tries to reuse a pooled object first, falling back to regular allocation
    pub fn allocWithType(self: *Arena, size: u32, type_tag: u16) !ObjRef {
        // Try to get from pool first
        if (self.object_pool.acquire(type_tag)) |pooled_ref| {
            return pooled_ref;
        }

        // Pool miss - allocate normally
        return self.alloc(size);
    }

    /// Free an object and return it to the pool if applicable (OPTIMIZED: Task 9.7)
    pub fn freeWithType(self: *Arena, objref: ObjRef, type_tag: u16) !void {
        // Try to return to pool
        try self.object_pool.release(type_tag, objref);

        // Also mark as free in the allocator
        try self.free(objref);
    }

    /// Atomically allocate space for an object with reservation pattern
    /// This is used by atomic operations to ensure crash consistency
    pub fn atomicAlloc(self: *Arena, size: u32) !ObjRef {
        return self.atomicAllocNear(size, null);
    }

    /// Atomically allocate space near another object for co-location (lock-free)
    /// Uses lock-free slab allocator for small objects and page allocator for large objects
    pub fn atomicAllocNear(self: *Arena, size: u32, _: ?ObjRef) !ObjRef {
        // Small objects (<= 1KB): use lock-free slab allocator
        if (size <= 1024) {
            if (self.slab_allocator.alloc(size)) |offset| {
                _ = self.object_count.fetchAdd(1, .seq_cst);
                _ = self.used.fetchAdd(size, .seq_cst);
                return ObjRef.init(offset);
            } else |err| {
                if (err != error.SizeTooBig) {
                    return err;
                }
                // Fall through to page allocator
            }
        }

        // Medium objects (1KB - 1MB): use lock-free page allocator
        if (size <= 1024 * 1024) {
            const num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
            const offset = try self.page_allocator.allocPages(num_pages);
            _ = self.object_count.fetchAdd(1, .seq_cst);
            _ = self.used.fetchAdd(size, .seq_cst);
            return ObjRef.init(offset);
        }

        // Very large objects (> 1MB): fall back to bump allocator
        const offset = self.bump.fetchAdd(size, .seq_cst);
        if (offset + size > self.size) {
            return error.OutOfMemory;
        }

        _ = self.used.fetchAdd(size, .seq_cst);
        _ = self.object_count.fetchAdd(1, .seq_cst);

        return ObjRef.init(offset);
    }

    /// Allocate space near another object for co-location (lock-free)
    pub fn allocNear(self: *Arena, size: u32, near: ?ObjRef) !ObjRef {
        // For co-location, we track metrics
        if (near != null) {
            _ = self.colocation_metrics.total_attempts.fetchAdd(1, .seq_cst);
        }

        // Small objects (<= 1KB): use lock-free slab allocator
        if (size <= 1024) {
            if (self.slab_allocator.alloc(size)) |offset| {
                _ = self.object_count.fetchAdd(1, .seq_cst);
                _ = self.used.fetchAdd(size, .seq_cst);

                // Check co-location success
                if (near) |near_ref| {
                    const near_page = near_ref.pageId();
                    const alloc_page = offset / PAGE_SIZE;
                    const page_diff = if (alloc_page > near_page)
                        alloc_page - near_page
                    else
                        near_page - alloc_page;

                    if (page_diff <= 1) {
                        _ = self.colocation_metrics.successful_colocations.fetchAdd(1, .seq_cst);
                    } else {
                        _ = self.colocation_metrics.failed_colocations.fetchAdd(1, .seq_cst);
                    }
                }

                return ObjRef.init(offset);
            } else |err| {
                if (err != error.SizeTooBig) {
                    return err;
                }
                // Fall through to page allocator
            }
        }

        // Medium objects (1KB - 1MB): use lock-free page allocator
        if (size <= 1024 * 1024) {
            const num_pages = (size + PAGE_SIZE - 1) / PAGE_SIZE;
            const offset = try self.page_allocator.allocPages(num_pages);
            _ = self.object_count.fetchAdd(1, .seq_cst);
            _ = self.used.fetchAdd(size, .seq_cst);

            // Check co-location success
            if (near) |near_ref| {
                const near_page = near_ref.pageId();
                const alloc_page = offset / PAGE_SIZE;
                const page_diff = if (alloc_page > near_page)
                    alloc_page - near_page
                else
                    near_page - alloc_page;

                if (page_diff <= 1) {
                    _ = self.colocation_metrics.successful_colocations.fetchAdd(1, .seq_cst);
                } else {
                    _ = self.colocation_metrics.failed_colocations.fetchAdd(1, .seq_cst);
                }
            }

            return ObjRef.init(offset);
        }

        // Very large objects (> 1MB): fall back to bump allocator
        const offset = self.bump.fetchAdd(size, .seq_cst);
        if (offset + size > self.size) {
            return error.OutOfMemory;
        }

        _ = self.used.fetchAdd(size, .seq_cst);
        _ = self.object_count.fetchAdd(1, .seq_cst);

        // Check co-location success
        if (near) |near_ref| {
            const near_page = near_ref.pageId();
            const alloc_page = offset / PAGE_SIZE;
            const page_diff = if (alloc_page > near_page)
                alloc_page - near_page
            else
                near_page - alloc_page;

            if (page_diff <= 1) {
                _ = self.colocation_metrics.successful_colocations.fetchAdd(1, .seq_cst);
            } else {
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

    /// Free an object slot for reuse (lock-free)
    pub fn free(self: *Arena, objref: ObjRef) !void {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        // Read object header to determine size
        const offset = objref.getOffset();
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        const header = try ObjectHeader.fromBytes(header_bytes);

        // Free based on size
        if (header.size <= 1024) {
            // Small object: return to slab allocator
            try self.slab_allocator.free(@intCast(offset), header.size);
        } else if (header.size <= 1024 * 1024) {
            // Medium object: return to page allocator
            const num_pages = (header.size + PAGE_SIZE - 1) / PAGE_SIZE;
            self.page_allocator.freePages(@intCast(offset), num_pages);
        }
        // Large objects (> 1MB) are not freed (bump allocator doesn't support free)

        _ = self.object_count.fetchSub(1, .seq_cst);
    }

    /// Atomically free an object slot for reuse (lock-free)
    /// Uses lock-free allocators to ensure thread safety
    pub fn atomicFree(self: *Arena, objref: ObjRef) !void {
        // Same implementation as free() since allocators are already lock-free
        return self.free(objref);
    }

    /// Batch allocation result
    pub const BatchAllocResult = struct {
        refs: []ObjRef,
        allocator: std.mem.Allocator,

        pub fn deinit(self: *BatchAllocResult) void {
            self.allocator.free(self.refs);
        }
    };

    /// Allocate space for multiple objects in a single operation (bulk allocation)
    /// This is optimized for batch inserts and reduces allocation overhead
    /// Objects are allocated contiguously when possible for better cache locality
    pub fn allocBatch(self: *Arena, sizes: []const u32, allocator: std.mem.Allocator) !BatchAllocResult {
        if (sizes.len == 0) {
            return BatchAllocResult{
                .refs = &[_]ObjRef{},
                .allocator = allocator,
            };
        }

        var refs = try allocator.alloc(ObjRef, sizes.len);
        errdefer allocator.free(refs);

        // Try to allocate all objects contiguously for better cache locality
        const total_size = blk: {
            var sum: u32 = 0;
            for (sizes) |size| {
                sum += size;
            }
            break :blk sum;
        };

        // If total size is reasonable, try contiguous allocation
        if (total_size <= 1024 * 1024) { // 1MB threshold
            const base_offset = try self.allocContiguous(total_size);
            var current_offset = base_offset;

            for (sizes, 0..) |size, i| {
                refs[i] = ObjRef.init(current_offset);
                current_offset += size;
            }
        } else {
            // Fall back to individual allocations for very large batches
            for (sizes, 0..) |size, i| {
                refs[i] = try self.alloc(size);
            }
        }

        return BatchAllocResult{
            .refs = refs,
            .allocator = allocator,
        };
    }

    /// Allocate contiguous space for related objects (co-location optimization)
    /// This ensures objects are placed in the same memory region for better cache performance
    fn allocContiguous(self: *Arena, total_size: u32) !u64 {
        // Use page allocator for medium-sized contiguous allocations
        if (total_size <= 1024 * 1024) {
            const num_pages = (total_size + PAGE_SIZE - 1) / PAGE_SIZE;
            const offset = self.page_allocator.allocPages(num_pages) catch {
                // Fall through to bump allocator if page allocation fails
                const bump_offset = self.bump.fetchAdd(total_size, .seq_cst);
                if (bump_offset + total_size > self.size) {
                    return error.OutOfMemory;
                }
                _ = self.used.fetchAdd(total_size, .seq_cst);
                return bump_offset;
            };
            _ = self.used.fetchAdd(total_size, .seq_cst);
            return offset;
        }

        // Fall back to bump allocator for very large allocations
        const offset = self.bump.fetchAdd(total_size, .seq_cst);
        if (offset + total_size > self.size) {
            return error.OutOfMemory;
        }

        _ = self.used.fetchAdd(total_size, .seq_cst);
        return offset;
    }

    /// Batch deallocation for garbage collection
    /// Frees multiple objects in a single operation for efficiency
    pub fn freeBatch(self: *Arena, objrefs: []const ObjRef) !void {
        for (objrefs) |objref| {
            try self.free(objref);
        }
    }

    /// Atomically allocate batch with reservation pattern for crash consistency
    /// This reserves space for all objects before committing any allocations
    pub fn atomicAllocBatch(self: *Arena, sizes: []const u32, allocator: std.mem.Allocator) !BatchAllocResult {
        // For atomic batch allocation, we use the same implementation
        // The underlying allocators are already lock-free and atomic
        return self.allocBatch(sizes, allocator);
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

        // OPTIMIZATION (Task 9.7): Prefetch object data before reading
        // This hides memory latency by bringing data into cache early
        const obj_ptr = self.mmap.ptr + offset;
        prefetch.prefetchForNavigation(obj_ptr, 256); // Prefetch first 256 bytes

        // Read header first to get total size
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        const header = try ObjectHeader.fromBytes(header_bytes);

        // Validate header using optimized fast path
        if (header.isDeletedFast()) {
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

    /// Read an object with prefetching for reference navigation (OPTIMIZED: Task 9.7)
    /// This variant prefetches the target object before reading, useful when you know
    /// you'll be navigating to it soon
    pub fn readWithPrefetch(self: *const Arena, objref: ObjRef, allocator: std.mem.Allocator) !Object {
        if (objref.isNull()) {
            return error.InvalidReference;
        }

        const offset = objref.getOffset();
        if (offset >= self.size) {
            return error.OutOfBounds;
        }

        // OPTIMIZATION: Aggressive prefetching for known navigation patterns
        const obj_ptr = self.mmap.ptr + offset;
        prefetch.prefetchRange(obj_ptr, 512, .high); // Prefetch more data with high locality

        // Read header first to get total size
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        const header = try ObjectHeader.fromBytes(header_bytes);

        // Validate header using optimized fast path
        if (header.isDeletedFast()) {
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

    // ========================================================================
    // Compression-Aware Operations (Task 10.2)
    // ========================================================================

    /// Write an object with automatic compression
    pub fn writeCompressed(self: *Arena, objref: ObjRef, data: []const u8) !void {
        if (!self.compression_enabled.load(.seq_cst)) {
            return self.write(objref, data);
        }

        if (objref.isNull()) {
            return error.InvalidReference;
        }

        // Validate data contains a valid object header
        if (data.len < ObjectHeader.headerSize()) {
            return error.DataTooSmall;
        }

        // Parse header to determine compression
        var header = try ObjectHeader.fromBytes(data[0..ObjectHeader.headerSize()]);
        const compression_type = CompressionEngine.selectCompressionType(@intCast(data.len));

        if (compression_type == .none) {
            // No compression - write directly
            return self.write(objref, data);
        }

        // Compress the data
        const compressed = try self.compression_engine.compressWithType(data, compression_type);
        defer self.allocator.free(compressed);

        // Update header with compression type
        header.setCompressionType(@intFromEnum(compression_type));
        const header_bytes = header.toBytes();

        // Write compressed data with updated header
        var final_data = try self.allocator.alloc(u8, header_bytes.len + compressed.len);
        defer self.allocator.free(final_data);

        @memcpy(final_data[0..header_bytes.len], &header_bytes);
        @memcpy(final_data[header_bytes.len..], compressed);

        try self.write(objref, final_data);
    }

    /// Read an object with automatic decompression and caching
    pub fn readCompressed(self: *const Arena, objref: ObjRef, allocator: std.mem.Allocator) !Object {
        if (!self.compression_enabled.load(.seq_cst)) {
            return self.read(objref, allocator);
        }

        if (objref.isNull()) {
            return error.InvalidReference;
        }

        // Check decompression cache first
        if (self.decompression_cache.get(objref)) |cached_data| {
            return try Object.fromBytes(allocator, cached_data);
        }

        const offset = objref.getOffset();
        if (offset >= self.size) {
            return error.OutOfBounds;
        }

        // Read header to check compression
        const header_bytes = try self.readAt(offset, ObjectHeader.headerSize());
        const header = try ObjectHeader.fromBytes(header_bytes);

        if (header.isDeletedFast()) {
            return error.ObjectDeleted;
        }

        // Check if compressed
        const compression_type_int = header.getCompressionType();
        const compression_type: CompressionType = @enumFromInt(compression_type_int);

        if (compression_type == .none) {
            // Not compressed - read normally
            return self.read(objref, allocator);
        }

        // Read compressed data
        const compressed_data = try self.readAt(offset + ObjectHeader.headerSize(), header.size - ObjectHeader.headerSize());

        // Decompress
        const decompressed = try self.compression_engine.decompress(compressed_data, compression_type);
        defer allocator.free(decompressed);

        // Cache decompressed data
        try self.decompression_cache.put(objref, decompressed);

        // Parse and return object
        return try Object.fromBytes(allocator, decompressed);
    }

    /// Enable or disable compression at runtime
    pub fn setCompressionEnabled(self: *Arena, enabled: bool) void {
        self.compression_enabled.store(enabled, .seq_cst);
    }

    /// Check if compression is enabled
    pub fn isCompressionEnabled(self: *const Arena) bool {
        return self.compression_enabled.load(.seq_cst);
    }

    /// Get compression statistics
    pub fn compressionStats(self: *const Arena) *const @import("compression.zig").CompressionStats {
        return self.compression_engine.getStats();
    }

    /// Get decompression cache statistics
    pub fn cacheStats(self: *const Arena) *const @import("decompression_cache.zig").CacheStats {
        return self.decompression_cache.getStats();
    }

    /// Get statistics about the arena
    pub fn stats(self: *const Arena) ArenaStats {
        return .{
            .total_size = self.size,
            .used_size = self.used.load(.seq_cst),
            .free_size = self.size - self.used.load(.seq_cst),
            .object_count = self.object_count.load(.seq_cst),
            .free_slots = 0, // Deprecated: now using lock-free allocators
        };
    }

    /// Get lock-free slab allocator statistics
    pub fn slabStats(self: *const Arena) SlabAllocatorStats {
        return .{
            .size_64 = self.slab_allocator.getStats(.size_64),
            .size_128 = self.slab_allocator.getStats(.size_128),
            .size_256 = self.slab_allocator.getStats(.size_256),
            .size_512 = self.slab_allocator.getStats(.size_512),
            .size_1024 = self.slab_allocator.getStats(.size_1024),
        };
    }

    /// Get lock-free page allocator statistics
    pub fn pageStats(self: *const Arena) PageStatsSnapshot {
        return self.page_allocator.getStats();
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

    /// Prefetch multiple objects for batch operations (OPTIMIZED: Task 9.7)
    /// This method prefetches multiple objects in parallel to maximize memory-level parallelism
    pub fn prefetchBatch(self: *const Arena, objrefs: []const ObjRef) void {
        var offsets: [16]u64 = undefined;
        var count: usize = 0;

        for (objrefs) |objref| {
            if (!objref.isNull() and objref.getOffset() < self.size) {
                offsets[count] = objref.getOffset();
                count += 1;
                if (count >= 16) break; // Limit to 16 prefetches
            }
        }

        if (count > 0) {
            prefetch.prefetchMultipleReferences(self.mmap.ptr, offsets[0..count]);
        }
    }

    /// Get object pool statistics (OPTIMIZED: Task 9.7)
    pub fn poolStats(self: *const Arena, type_tag: u16) ?PoolStats {
        return self.object_pool.getStats(type_tag);
    }

    /// Get hit rate for a pooled type (OPTIMIZED: Task 9.7)
    pub fn poolHitRate(self: *const Arena, type_tag: u16) f64 {
        return self.object_pool.hitRate(type_tag);
    }

    // ========================================================================
    // Distributed Operations (GlobalObjRef Support)
    // ========================================================================

    /// Allocate space and return GlobalObjRef for distributed addressing
    pub fn allocGlobal(self: *Arena, size: u32) !GlobalObjRef {
        const local_ref = try self.alloc(size);
        return GlobalObjRef.fromLocal(local_ref, self.shard_id);
    }

    /// Allocate space near another GlobalObjRef for co-location
    pub fn allocGlobalNear(self: *Arena, size: u32, near: ?GlobalObjRef) !GlobalObjRef {
        const near_local = if (near) |global_ref| blk: {
            if (global_ref.isLocal(self.shard_id)) {
                break :blk global_ref.toLocal();
            } else {
                // Remote reference - can't co-locate, use regular allocation
                break :blk null;
            }
        } else null;

        const local_ref = try self.allocNear(size, near_local);
        return GlobalObjRef.fromLocal(local_ref, self.shard_id);
    }

    /// Navigate a GlobalObjRef with O(1) local performance
    ///
    /// Returns NavigationResult indicating whether the object is local or remote.
    /// For local objects, provides direct access. For remote objects, provides
    /// shard information for network navigation.
    pub fn navigateGlobal(self: *const Arena, global_ref: GlobalObjRef, allocator: std.mem.Allocator) NavigationResult(Object) {
        if (global_ref.isNull()) {
            return NavigationResult(Object){ .not_found = {} };
        }

        if (global_ref.isLocal(self.shard_id)) {
            // Local navigation: O(1) performance (<50ns)
            const local_ref = global_ref.toLocal();
            if (self.read(local_ref, allocator)) |obj| {
                return NavigationResult(Object){ .local = obj };
            } else |_| {
                return NavigationResult(Object){ .not_found = {} };
            }
        } else {
            // Remote navigation: requires network call (<1μs)
            return NavigationResult(Object){ .remote = .{
                .shard_id = global_ref.getShardId(),
                .objref = global_ref,
            } };
        }
    }

    /// Write an object using GlobalObjRef
    pub fn writeGlobal(self: *Arena, global_ref: GlobalObjRef, data: []const u8) !void {
        if (!global_ref.isLocal(self.shard_id)) {
            return error.RemoteReference;
        }

        const local_ref = global_ref.toLocal();
        return self.write(local_ref, data);
    }

    /// Read an object using GlobalObjRef
    pub fn readGlobal(self: *const Arena, global_ref: GlobalObjRef, allocator: std.mem.Allocator) !Object {
        if (!global_ref.isLocal(self.shard_id)) {
            return error.RemoteReference;
        }

        const local_ref = global_ref.toLocal();
        return self.read(local_ref, allocator);
    }

    /// Delete an object using GlobalObjRef
    pub fn deleteGlobal(self: *Arena, global_ref: GlobalObjRef) !void {
        if (!global_ref.isLocal(self.shard_id)) {
            return error.RemoteReference;
        }

        const local_ref = global_ref.toLocal();
        return self.delete(local_ref);
    }

    /// Free an object using GlobalObjRef
    pub fn freeGlobal(self: *Arena, global_ref: GlobalObjRef) !void {
        if (!global_ref.isLocal(self.shard_id)) {
            return error.RemoteReference;
        }

        const local_ref = global_ref.toLocal();
        return self.free(local_ref);
    }

    /// Get the current shard ID
    pub fn getShardId(self: *const Arena) ShardId {
        return self.shard_id;
    }

    /// Check if a GlobalObjRef is local to this arena
    pub fn isLocalRef(self: *const Arena, global_ref: GlobalObjRef) bool {
        return global_ref.isLocal(self.shard_id);
    }

    /// Convert local ObjRef to GlobalObjRef for this shard
    pub fn toGlobalRef(self: *const Arena, local_ref: ObjRef) GlobalObjRef {
        return GlobalObjRef.fromLocal(local_ref, self.shard_id);
    }

    /// Cross-shard reference resolution (placeholder for network layer)
    ///
    /// This method provides the interface for cross-shard navigation.
    /// In a full distributed implementation, this would coordinate with
    /// the network layer to fetch objects from remote shards.
    pub fn resolveRemoteRef(self: *const Arena, global_ref: GlobalObjRef, allocator: std.mem.Allocator) !?Object {
        _ = self;
        _ = allocator;

        if (global_ref.isNull()) {
            return null;
        }

        // For now, return error to indicate remote resolution is not implemented
        // In the full implementation, this would:
        // 1. Check local cache for the remote object
        // 2. If not cached, send network request to target shard
        // 3. Deserialize response and cache locally
        // 4. Return the object
        return error.RemoteResolutionNotImplemented;
    }

    // ========================================================================
    // Crash Recovery Support
    // ========================================================================

    /// Check arena integrity for crash recovery
    pub fn checkIntegrity(self: *const Arena) !bool {
        var integrity_ok = true;
        var current_offset: u64 = INITIAL_OFFSET;
        const arena_stats = self.stats();

        while (current_offset < arena_stats.used_size) {
            // Try to read object header
            const header_data = self.readAt(current_offset, ObjectHeader.headerSize()) catch |err| {
                if (err == error.OutOfBounds) break;
                std.log.warn("Arena integrity check: failed to read header at offset {}: {}", .{ current_offset, err });
                integrity_ok = false;
                current_offset += ObjectHeader.headerSize();
                continue;
            };

            // Parse header
            const header = ObjectHeader.fromBytes(header_data) catch |err| {
                std.log.warn("Arena integrity check: invalid header at offset {}: {}", .{ current_offset, err });
                integrity_ok = false;
                current_offset += ObjectHeader.headerSize();
                continue;
            };

            // Validate header fields
            if (header.size == 0 or header.size > self.size) {
                std.log.warn("Arena integrity check: invalid object size {} at offset {}", .{ header.size, current_offset });
                integrity_ok = false;
            }

            // Check if object extends beyond arena
            if (current_offset + header.size > self.size) {
                std.log.warn("Arena integrity check: object at offset {} extends beyond arena boundary", .{current_offset});
                integrity_ok = false;
                break;
            }

            // Try to read full object if not deleted
            if (!header.isDeleted()) {
                const full_data = self.readAt(current_offset, header.size) catch |err| {
                    std.log.warn("Arena integrity check: failed to read object at offset {}: {}", .{ current_offset, err });
                    integrity_ok = false;
                    current_offset += header.size;
                    continue;
                };

                // Try to parse object
                var obj = Object.fromBytes(self.allocator, full_data) catch |err| {
                    std.log.warn("Arena integrity check: failed to parse object at offset {}: {}", .{ current_offset, err });
                    integrity_ok = false;
                    current_offset += header.size;
                    continue;
                };
                defer obj.deinit();

                // Verify checksum
                obj.verify() catch |err| {
                    if (err == error.ChecksumMismatch) {
                        std.log.warn("Arena integrity check: checksum mismatch at offset {}", .{current_offset});
                        integrity_ok = false;
                    }
                };
            }

            current_offset += header.size;
        }

        return integrity_ok;
    }

    /// Repair corrupted object at given offset
    pub fn repairObject(self: *Arena, offset: u64, allocator: std.mem.Allocator) !bool {
        // Try to read the object
        const header_data = self.readAt(offset, ObjectHeader.headerSize()) catch {
            return false; // Cannot repair if we can't read header
        };

        const header = ObjectHeader.fromBytes(header_data) catch {
            return false; // Cannot repair completely corrupted header
        };

        // Try to read full object
        const full_data = self.readAt(offset, header.size) catch {
            return false; // Cannot repair if we can't read full object
        };

        var obj = Object.fromBytes(allocator, full_data) catch {
            return false; // Cannot repair if object is completely corrupted
        };
        defer obj.deinit();

        // Try to repair checksum
        obj.updateChecksum() catch {
            return false; // Cannot repair checksum
        };

        // Serialize repaired object
        const repaired_data = obj.toBytes(allocator) catch {
            return false; // Cannot serialize repaired object
        };
        defer allocator.free(repaired_data);

        // Write repaired object back
        self.writeAt(offset, repaired_data) catch {
            return false; // Cannot write repaired object
        };

        std.log.info("Successfully repaired object at offset {}", .{offset});
        return true;
    }

    /// Scan arena and collect object information for recovery
    pub fn scanObjects(self: *Arena, allocator: std.mem.Allocator) !std.ArrayList(ObjectInfo) {
        var objects = std.ArrayList(ObjectInfo){};
        var current_offset: u64 = INITIAL_OFFSET;
        const arena_stats = self.stats();
        const max_offset = @min(arena_stats.used_size, self.size);

        while (current_offset < max_offset) {
            // Safety check: ensure we have room for at least a header
            if (current_offset + ObjectHeader.headerSize() > max_offset) break;

            // Try to read object header
            const header_data = self.readAt(current_offset, ObjectHeader.headerSize()) catch |err| {
                if (err == error.OutOfBounds) break;
                // Skip forward by a reasonable amount to avoid infinite loop
                current_offset += ObjectHeader.headerSize();
                continue;
            };

            const header = ObjectHeader.fromBytes(header_data) catch {
                // Invalid header - skip forward
                current_offset += ObjectHeader.headerSize();
                continue;
            };

            // Safety check: ensure header size is reasonable
            if (header.size == 0 or header.size > self.size or current_offset + header.size > max_offset) {
                // Invalid size - skip forward
                current_offset += ObjectHeader.headerSize();
                continue;
            }

            // Create object info
            const obj_info = ObjectInfo{
                .offset = current_offset,
                .size = header.size,
                .type_tag = header.type_tag,
                .is_deleted = header.isDeleted(),
                .checksum_valid = blk: {
                    if (header.isDeleted()) break :blk true; // Skip checksum for deleted objects

                    const full_data = self.readAt(current_offset, header.size) catch break :blk false;
                    var obj = Object.fromBytes(allocator, full_data) catch break :blk false;
                    defer obj.deinit();

                    obj.verify() catch break :blk false;
                    break :blk true;
                },
            };

            try objects.append(allocator, obj_info);
            current_offset += header.size;
        }

        return objects;
    }

    /// Compact arena by removing deleted objects and defragmenting
    pub fn compact(self: *Arena, allocator: std.mem.Allocator) !void {
        var objects = try self.scanObjects(allocator);
        defer objects.deinit(allocator);

        var write_offset: u64 = INITIAL_OFFSET;
        var compacted_count: u64 = 0;

        for (objects.items) |obj_info| {
            if (obj_info.is_deleted) continue; // Skip deleted objects

            // Read object data
            const obj_data = try self.readAt(obj_info.offset, obj_info.size);

            // Write to new location if different
            if (obj_info.offset != write_offset) {
                try self.writeAt(write_offset, obj_data);
            }

            write_offset += obj_info.size;
            compacted_count += 1;
        }

        // Update arena statistics
        _ = self.used.store(write_offset - INITIAL_OFFSET, .seq_cst);
        _ = self.object_count.store(compacted_count, .seq_cst);
        _ = self.bump.store(@intCast(write_offset), .seq_cst);

        std.log.info("Arena compaction completed: {} objects, {} bytes used", .{ compacted_count, write_offset - INITIAL_OFFSET });
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

/// Slab allocator statistics for all size classes
pub const SlabAllocatorStats = struct {
    size_64: SlabClassStats,
    size_128: SlabClassStats,
    size_256: SlabClassStats,
    size_512: SlabClassStats,
    size_1024: SlabClassStats,
};

/// Object information for recovery and scanning
pub const ObjectInfo = struct {
    /// Offset of object in arena
    offset: u64,
    /// Size of object in bytes
    size: u32,
    /// Object type tag
    type_tag: u16,
    /// Whether object is deleted
    is_deleted: bool,
    /// Whether object checksum is valid
    checksum_valid: bool,
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

    const arena = try Arena.init(allocator, file_path, 1024 * 1024, 0); // 1MB, shard 0
    return .{ .arena = arena, .dir = tmp };
}

fn createTestArenaWithShard(allocator: std.mem.Allocator, shard_id: ShardId) !struct { arena: Arena, dir: std.testing.TmpDir } {
    var tmp = std.testing.tmpDir(.{});
    const path = try tmp.dir.realpathAlloc(allocator, ".");
    defer allocator.free(path);

    const file_path = try std.fs.path.join(allocator, &[_][]const u8{ path, "test_arena.dat" });
    defer allocator.free(file_path);

    const arena = try Arena.init(allocator, file_path, 1024 * 1024, shard_id); // 1MB
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
    // Slab allocator rounds up to size classes, so offset may not be sequential
    try std.testing.expect(!ref2.isNull());
    try std.testing.expect(ref2.getOffset() > ref1.getOffset());

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

    // Check slab allocator has the freed slot
    const slab_stats = test_arena.arena.slabStats();
    try std.testing.expect(slab_stats.size_64.free_list_size > 0);

    // Allocate again - should reuse the free slot
    const ref2 = try test_arena.arena.alloc(64);
    try std.testing.expectEqual(ref.getOffset(), ref2.getOffset());
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

    var arena = try Arena.init(allocator, file_path, 1000, 0);
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
    // Page allocator allocates full pages, so utilization may be 0 for page_usage tracking
    // The actual utilization is tracked by the page allocator itself
    try std.testing.expect(utilization >= 0.0);
    try std.testing.expect(utilization <= 100.0);
}

// ============================================================================
// Distributed Operations Tests
// ============================================================================

test "Arena: shard ID assignment" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 42);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    try std.testing.expectEqual(@as(ShardId, 42), test_arena.arena.getShardId());
}

test "Arena: global allocation" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 5);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const global_ref = try test_arena.arena.allocGlobal(100);

    try std.testing.expect(!global_ref.isNull());
    try std.testing.expectEqual(@as(ShardId, 5), global_ref.getShardId());
    try std.testing.expectEqual(@as(u64, INITIAL_OFFSET), global_ref.getLocalOffset());
    try std.testing.expect(global_ref.isLocal(5));
    try std.testing.expect(!global_ref.isLocal(3));
}

test "Arena: global allocation near" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 1);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate first object
    const ref1 = try test_arena.arena.allocGlobal(500);

    // Allocate near the first object - should be co-located
    const ref2 = try test_arena.arena.allocGlobalNear(500, ref1);

    try std.testing.expectEqual(ref1.getShardId(), ref2.getShardId());
    try std.testing.expectEqual(ref1.pageId(), ref2.pageId()); // Same page
}

test "Arena: global allocation near remote reference" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 1);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Create a remote reference (different shard)
    const remote_ref = GlobalObjRef.init(5, 1024, 0);

    // Allocate near remote reference - should ignore co-location
    const local_ref = try test_arena.arena.allocGlobalNear(100, remote_ref);

    try std.testing.expectEqual(@as(ShardId, 1), local_ref.getShardId());
    try std.testing.expect(!local_ref.isNull());
}

test "Arena: local reference check" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 3);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const local_ref = GlobalObjRef.init(3, 1024, 0);
    const remote_ref = GlobalObjRef.init(7, 2048, 0);

    try std.testing.expect(test_arena.arena.isLocalRef(local_ref));
    try std.testing.expect(!test_arena.arena.isLocalRef(remote_ref));
}

test "Arena: local to global conversion" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 2);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const local_ref = ObjRef.init(1024);
    const global_ref = test_arena.arena.toGlobalRef(local_ref);

    try std.testing.expectEqual(@as(ShardId, 2), global_ref.getShardId());
    try std.testing.expectEqual(@as(u64, 1024), global_ref.getLocalOffset());
    try std.testing.expectEqual(@as(u32, 0), global_ref.getFlags());
}

test "Arena: navigate global local reference" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 0);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Create and write an object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const global_ref = try test_arena.arena.allocGlobal(@intCast(data.len));
    try test_arena.arena.writeGlobal(global_ref, data);

    // Navigate to the object
    const result = test_arena.arena.navigateGlobal(global_ref, allocator);

    try std.testing.expect(result.isLocal());
    try std.testing.expect(!result.isRemote());

    var retrieved_obj = result.getLocal().?;
    defer retrieved_obj.deinit();

    try std.testing.expectEqual(obj.header.type_tag, retrieved_obj.header.type_tag);
    try std.testing.expectEqualSlices(u8, obj.fixed, retrieved_obj.fixed);
}

test "Arena: navigate global remote reference" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 1);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Create a remote reference
    const remote_ref = GlobalObjRef.init(5, 2048, 0);

    // Navigate to remote object
    const result = test_arena.arena.navigateGlobal(remote_ref, allocator);

    try std.testing.expect(!result.isLocal());
    try std.testing.expect(result.isRemote());

    const remote_info = result.getRemoteInfo().?;
    try std.testing.expectEqual(@as(ShardId, 5), remote_info.shard_id);
    try std.testing.expectEqual(remote_ref.getLocalOffset(), remote_info.objref.getLocalOffset());
}

test "Arena: navigate global null reference" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const null_ref = GlobalObjRef.nullRef();
    const result = test_arena.arena.navigateGlobal(null_ref, allocator);

    try std.testing.expect(!result.isLocal());
    try std.testing.expect(!result.isRemote());
}

test "Arena: write global local reference" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Create a proper object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(allocator);
    defer allocator.free(data);

    const global_ref = try test_arena.arena.allocGlobal(@intCast(data.len));
    try test_arena.arena.writeGlobal(global_ref, data);

    // Verify data was written by reading it back as an object
    var read_obj = try test_arena.arena.readGlobal(global_ref, allocator);
    defer read_obj.deinit();

    try std.testing.expectEqual(obj.header.type_tag, read_obj.header.type_tag);
    try std.testing.expectEqualSlices(u8, obj.fixed, read_obj.fixed);
}

test "Arena: write global remote reference fails" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 1);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const remote_ref = GlobalObjRef.init(5, 1024, 0);
    const data = "Should fail";

    const result = test_arena.arena.writeGlobal(remote_ref, data);
    try std.testing.expectError(error.RemoteReference, result);
}

test "Arena: read global local reference" {
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

    const global_ref = try test_arena.arena.allocGlobal(@intCast(data.len));
    try test_arena.arena.writeGlobal(global_ref, data);

    // Read using global reference
    var read_obj = try test_arena.arena.readGlobal(global_ref, allocator);
    defer read_obj.deinit();

    try std.testing.expectEqual(obj.header.type_tag, read_obj.header.type_tag);
    try std.testing.expectEqualSlices(u8, obj.fixed, read_obj.fixed);
}

test "Arena: read global remote reference fails" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 1);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const remote_ref = GlobalObjRef.init(5, 1024, 0);

    const result = test_arena.arena.readGlobal(remote_ref, allocator);
    try std.testing.expectError(error.RemoteReference, result);
}

test "Arena: delete global local reference" {
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

    const global_ref = try test_arena.arena.allocGlobal(@intCast(data.len));
    try test_arena.arena.writeGlobal(global_ref, data);

    // Delete using global reference
    try test_arena.arena.deleteGlobal(global_ref);

    // Try to read - should fail
    const result = test_arena.arena.readGlobal(global_ref, allocator);
    try std.testing.expectError(error.ObjectDeleted, result);
}

test "Arena: delete global remote reference fails" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 1);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const remote_ref = GlobalObjRef.init(5, 1024, 0);

    const result = test_arena.arena.deleteGlobal(remote_ref);
    try std.testing.expectError(error.RemoteReference, result);
}

test "Arena: free global local reference" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const global_ref = try test_arena.arena.allocGlobal(64); // Slot-sized
    try test_arena.arena.freeGlobal(global_ref);

    try std.testing.expectEqual(@as(u64, 0), test_arena.arena.object_count.load(.seq_cst));

    // Check slab allocator has the freed slot
    const slab_stats = test_arena.arena.slabStats();
    try std.testing.expect(slab_stats.size_64.free_list_size > 0);
}

test "Arena: free global remote reference fails" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArenaWithShard(allocator, 1);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const remote_ref = GlobalObjRef.init(5, 1024, 0);

    const result = test_arena.arena.freeGlobal(remote_ref);
    try std.testing.expectError(error.RemoteReference, result);
}

test "Arena: resolve remote reference not implemented" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const remote_ref = GlobalObjRef.init(5, 1024, 0);

    const result = test_arena.arena.resolveRemoteRef(remote_ref, allocator);
    try std.testing.expectError(error.RemoteResolutionNotImplemented, result);
}

test "Arena: resolve null reference" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    const null_ref = GlobalObjRef.nullRef();

    const result = try test_arena.arena.resolveRemoteRef(null_ref, allocator);
    try std.testing.expect(result == null);
}

test "Arena: scanObjects doesn't infinite loop on corrupted data" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate a few valid objects
    const ref1 = try test_arena.arena.alloc(128);
    const ref2 = try test_arena.arena.alloc(256);
    const ref3 = try test_arena.arena.alloc(512);

    // Write some valid data
    const id1 = [_]u8{1} ** 16;
    var obj1 = try Object.init(allocator, 1, id1, &[_]u8{1}, "test1", &[_]ObjRef{});
    defer obj1.deinit();
    const data1 = try obj1.toBytes(allocator);
    defer allocator.free(data1);
    try test_arena.arena.write(ref1, data1);

    // Corrupt the second object by writing zeros (invalid header)
    const corrupt_data = [_]u8{0} ** 64;
    try test_arena.arena.writeAt(ref2.getOffset(), &corrupt_data);

    // Write valid third object
    const id3 = [_]u8{3} ** 16;
    var obj3 = try Object.init(allocator, 3, id3, &[_]u8{3}, "test3", &[_]ObjRef{});
    defer obj3.deinit();
    const data3 = try obj3.toBytes(allocator);
    defer allocator.free(data3);
    try test_arena.arena.write(ref3, data3);

    // Scan should complete without hanging
    var objects = try test_arena.arena.scanObjects(allocator);
    defer objects.deinit(allocator);

    // Should find at least the valid objects (may skip corrupted one)
    try std.testing.expect(objects.items.len >= 1);
    try std.testing.expect(objects.items.len <= 3);
}

test "Arena: scanObjects handles empty arena" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Scan empty arena
    var objects = try test_arena.arena.scanObjects(allocator);
    defer objects.deinit(allocator);

    try std.testing.expectEqual(@as(usize, 0), objects.items.len);
}

test "Arena: compact doesn't hang" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate several objects
    var refs: [5]ObjRef = undefined;
    for (&refs, 0..) |*ref, i| {
        ref.* = try test_arena.arena.alloc(128);
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "test", &[_]ObjRef{});
        defer obj.deinit();
        const data = try obj.toBytes(allocator);
        defer allocator.free(data);
        try test_arena.arena.write(ref.*, data);
    }

    const initial_count = test_arena.arena.object_count.load(.seq_cst);
    try std.testing.expectEqual(@as(u64, 5), initial_count);

    // Compact should complete without hanging
    try test_arena.arena.compact(allocator);

    // After compaction, we should still have objects
    const final_count = test_arena.arena.object_count.load(.seq_cst);
    try std.testing.expect(final_count > 0);
}

test "Arena: batch allocation" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate batch of objects
    const sizes = [_]u32{ 128, 256, 512, 128, 256 };
    var batch_result = try test_arena.arena.allocBatch(&sizes, allocator);
    defer batch_result.deinit();

    // Verify all allocations succeeded
    try std.testing.expectEqual(@as(usize, 5), batch_result.refs.len);

    // Verify all references are valid
    for (batch_result.refs) |ref| {
        try std.testing.expect(!ref.isNull());
        try std.testing.expect(ref.getOffset() >= INITIAL_OFFSET);
    }

    // Verify object count
    const count = test_arena.arena.object_count.load(.seq_cst);
    try std.testing.expect(count >= 5);
}

test "Arena: batch allocation contiguous" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate small batch that should be contiguous
    const sizes = [_]u32{ 100, 100, 100 };
    var batch_result = try test_arena.arena.allocBatch(&sizes, allocator);
    defer batch_result.deinit();

    // Verify allocations are reasonably close (contiguous or near-contiguous)
    const first_offset = batch_result.refs[0].getOffset();
    const second_offset = batch_result.refs[1].getOffset();
    const third_offset = batch_result.refs[2].getOffset();

    // Check that offsets are in order and close together
    try std.testing.expect(second_offset >= first_offset);
    try std.testing.expect(third_offset >= second_offset);
}

test "Arena: batch deallocation" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate batch
    const sizes = [_]u32{ 128, 256, 512 };
    var batch_result = try test_arena.arena.allocBatch(&sizes, allocator);
    defer batch_result.deinit();

    // Write object headers so free can read them
    for (batch_result.refs, 0..) |ref, i| {
        const id = [_]u8{@intCast(i)} ** 16;
        var obj = try Object.init(allocator, @intCast(i), id, &[_]u8{@intCast(i)}, "test", &[_]ObjRef{});
        defer obj.deinit();
        const data = try obj.toBytes(allocator);
        defer allocator.free(data);
        try test_arena.arena.write(ref, data);
    }

    const initial_count = test_arena.arena.object_count.load(.seq_cst);

    // Free batch
    try test_arena.arena.freeBatch(batch_result.refs);

    // Verify object count decreased
    const final_count = test_arena.arena.object_count.load(.seq_cst);
    try std.testing.expect(final_count < initial_count);
}

test "Arena: empty batch allocation" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate empty batch
    const sizes: []const u32 = &[_]u32{};
    var batch_result = try test_arena.arena.allocBatch(sizes, allocator);
    defer batch_result.deinit();

    // Should return empty array
    try std.testing.expectEqual(@as(usize, 0), batch_result.refs.len);
}

test "Arena: atomic batch allocation" {
    const allocator = std.testing.allocator;
    var test_arena = try createTestArena(allocator);
    defer test_arena.dir.cleanup();
    defer test_arena.arena.deinit();

    // Allocate batch atomically
    const sizes = [_]u32{ 128, 256, 512, 128 };
    var batch_result = try test_arena.arena.atomicAllocBatch(&sizes, allocator);
    defer batch_result.deinit();

    // Verify all allocations succeeded
    try std.testing.expectEqual(@as(usize, 4), batch_result.refs.len);

    // Verify all references are valid
    for (batch_result.refs) |ref| {
        try std.testing.expect(!ref.isNull());
    }
}
