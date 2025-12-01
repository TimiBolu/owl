// Task 10.4: Garbage Collection
// Mark-and-sweep garbage collector for reclaiming deleted objects

const std = @import("std");
const Arena = @import("arena.zig").Arena;
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;

/// Garbage collection statistics
pub const GCStats = struct {
    /// Number of objects scanned
    objects_scanned: usize,
    /// Number of objects marked as reachable
    objects_marked: usize,
    /// Number of objects swept (deleted)
    objects_swept: usize,
    /// Bytes reclaimed
    bytes_reclaimed: usize,
    /// Duration of GC cycle in nanoseconds
    duration_ns: u64,
    /// Number of GC cycles completed
    cycles_completed: usize,

    pub fn init() GCStats {
        return .{
            .objects_scanned = 0,
            .objects_marked = 0,
            .objects_swept = 0,
            .bytes_reclaimed = 0,
            .duration_ns = 0,
            .cycles_completed = 0,
        };
    }
};

/// Garbage collector for arena-based storage
pub const GarbageCollector = struct {
    /// Arena being managed
    arena: *Arena,
    /// Mark bitmap (one bit per slot)
    mark_bitmap: std.DynamicBitSet,
    /// Root set of objects (always reachable)
    roots: std.ArrayList(ObjRef),
    /// GC statistics
    stats: GCStats,
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    /// Whether GC is currently running
    running: std.atomic.Value(bool),

    /// Create a new garbage collector
    pub fn init(allocator: std.mem.Allocator, arena: *Arena) !GarbageCollector {
        // Calculate number of slots in arena (64 bytes per slot)
        const num_slots = arena.size / 64;

        return .{
            .arena = arena,
            .mark_bitmap = try std.DynamicBitSet.initEmpty(allocator, num_slots),
            .roots = std.ArrayList(ObjRef){},
            .stats = GCStats.init(),
            .allocator = allocator,
            .running = std.atomic.Value(bool).init(false),
        };
    }

    /// Clean up garbage collector resources
    pub fn deinit(self: *GarbageCollector) void {
        self.mark_bitmap.deinit();
        self.roots.deinit(self.allocator);
    }

    /// Add a root object (always reachable)
    pub fn addRoot(self: *GarbageCollector, root: ObjRef) !void {
        if (!root.isNull()) {
            try self.roots.append(self.allocator, root);
        }
    }

    /// Remove a root object
    pub fn removeRoot(self: *GarbageCollector, root: ObjRef) void {
        for (self.roots.items, 0..) |r, i| {
            if (r.getOffset() == root.getOffset()) {
                _ = self.roots.swapRemove(i);
                return;
            }
        }
    }

    /// Clear all roots
    pub fn clearRoots(self: *GarbageCollector) void {
        self.roots.clearRetainingCapacity();
    }

    /// Run a full garbage collection cycle
    pub fn collect(self: *GarbageCollector) !void {
        // Check if GC is already running
        if (self.running.swap(true, .seq_cst)) {
            return error.GCAlreadyRunning;
        }
        defer self.running.store(false, .seq_cst);

        const start_time = std.time.nanoTimestamp();

        // Reset statistics for this cycle
        var cycle_stats = GCStats.init();

        // Phase 1: Mark
        try self.mark(&cycle_stats);

        // Phase 2: Sweep
        try self.sweep(&cycle_stats);

        // Update statistics
        const end_time = std.time.nanoTimestamp();
        cycle_stats.duration_ns = @intCast(end_time - start_time);
        cycle_stats.cycles_completed = self.stats.cycles_completed + 1;

        // Accumulate stats
        self.stats.objects_scanned += cycle_stats.objects_scanned;
        self.stats.objects_marked += cycle_stats.objects_marked;
        self.stats.objects_swept += cycle_stats.objects_swept;
        self.stats.bytes_reclaimed += cycle_stats.bytes_reclaimed;
        self.stats.duration_ns = cycle_stats.duration_ns;
        self.stats.cycles_completed = cycle_stats.cycles_completed;
    }

    /// Mark phase: mark all reachable objects
    fn mark(self: *GarbageCollector, cycle_stats: *GCStats) !void {
        // Clear mark bitmap
        self.mark_bitmap.setRangeValue(.{ .start = 0, .end = self.mark_bitmap.capacity() }, false);

        // Mark from each root
        for (self.roots.items) |root| {
            try self.markObject(root, cycle_stats);
        }
    }

    /// Recursively mark an object and its references
    fn markObject(self: *GarbageCollector, objref: ObjRef, cycle_stats: *GCStats) !void {
        if (objref.isNull()) return;

        // Calculate slot index (64 bytes per slot)
        const slot_idx = objref.getOffset() / 64;
        if (slot_idx >= self.mark_bitmap.capacity()) return;

        // Already marked?
        if (self.mark_bitmap.isSet(slot_idx)) return;

        // Try to read object header
        const header_bytes = self.arena.readAt(objref.getOffset(), ObjectHeader.headerSize()) catch return;
        const header = ObjectHeader.fromBytes(header_bytes) catch return;

        // Skip deleted objects
        if (header.isDeleted()) return;

        // Mark this object
        self.mark_bitmap.set(slot_idx);
        cycle_stats.objects_marked += 1;

        // Read full object to get references
        var obj = self.arena.read(objref, self.allocator) catch return;
        defer obj.deinit();

        // Recursively mark referenced objects
        for (obj.refs) |ref| {
            try self.markObject(ref, cycle_stats);
        }
    }

    /// Sweep phase: reclaim unmarked objects
    fn sweep(self: *GarbageCollector, cycle_stats: *GCStats) !void {
        // Scan through all allocated objects
        var offset: u64 = 64; // INITIAL_OFFSET

        while (offset < self.arena.size) {
            cycle_stats.objects_scanned += 1;

            // Try to read object header
            const header_bytes = self.arena.readAt(offset, ObjectHeader.headerSize()) catch break;
            const header = ObjectHeader.fromBytes(header_bytes) catch {
                offset += 64; // SLOT_SIZE
                continue;
            };

            // Calculate slot index
            const slot_idx = offset / 64; // SLOT_SIZE

            // If not marked and not already deleted, sweep it
            if (slot_idx < self.mark_bitmap.capacity() and
                !self.mark_bitmap.isSet(slot_idx) and
                !header.isDeleted())
            {
                // Mark as deleted
                const objref = ObjRef.init(offset);
                self.arena.delete(objref) catch {};

                cycle_stats.objects_swept += 1;
                cycle_stats.bytes_reclaimed += header.size;
            }

            // Move to next object
            if (header.size > 0) {
                offset += header.size;
            } else {
                offset += 64; // SLOT_SIZE
            }
        }
    }

    /// Compact arena by moving live objects together
    /// This is optional and expensive, but reduces fragmentation
    pub fn compact(_: *GarbageCollector) !void {
        // TODO: Implement compaction
        // This is complex because it requires updating all references
        // For now, we just rely on the free list for space reuse
        return error.NotImplemented;
    }

    /// Get current GC statistics
    pub fn getStats(self: *const GarbageCollector) GCStats {
        return self.stats;
    }

    /// Check if GC is currently running
    pub fn isRunning(self: *const GarbageCollector) bool {
        return self.running.load(.seq_cst);
    }
};

/// Background GC thread manager
pub const BackgroundGC = struct {
    /// Garbage collector
    gc: *GarbageCollector,
    /// Thread handle
    thread: ?std.Thread,
    /// Whether to stop the background thread
    should_stop: std.atomic.Value(bool),
    /// Interval between GC cycles in milliseconds
    interval_ms: u64,

    /// Create a new background GC manager
    pub fn init(_: std.mem.Allocator, gc: *GarbageCollector, interval_ms: u64) BackgroundGC {
        return .{
            .gc = gc,
            .thread = null,
            .should_stop = std.atomic.Value(bool).init(false),
            .interval_ms = interval_ms,
        };
    }

    /// Start the background GC thread
    pub fn start(self: *BackgroundGC) !void {
        if (self.thread != null) {
            return error.AlreadyStarted;
        }

        self.should_stop.store(false, .seq_cst);
        self.thread = try std.Thread.spawn(.{}, runGCLoop, .{self});
    }

    /// Stop the background GC thread
    pub fn stop(self: *BackgroundGC) void {
        if (self.thread) |thread| {
            self.should_stop.store(true, .seq_cst);
            thread.join();
            self.thread = null;
        }
    }

    /// Background GC loop
    fn runGCLoop(self: *BackgroundGC) void {
        while (!self.should_stop.load(.seq_cst)) {
            // Run GC cycle
            self.gc.collect() catch |err| {
                std.debug.print("GC error: {}\n", .{err});
            };

            // Sleep for interval
            std.Thread.sleep(self.interval_ms * std.time.ns_per_ms);
        }
    }
};
