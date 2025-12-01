//! Lock-Free Reference Counting with Epoch-Based Memory Reclamation
//!
//! This module implements a lock-free reference counting system for shared objects
//! with epoch-based memory reclamation for safe cleanup. This enables high-performance
//! concurrent access to shared objects without locks while ensuring memory safety.
//!
//! Key Features:
//! - Atomic reference counting for shared objects
//! - Epoch-based memory reclamation (EBR) for safe cleanup
//! - Lock-free garbage collection for unreferenced objects
//! - Thread-safe memory management without blocking
//!
//! Design:
//! - Objects have atomic reference counts in their headers
//! - When refcount reaches 0, objects are retired to an epoch-based reclamation queue
//! - Epochs track which threads are accessing which generation of objects
//! - Objects are only freed when no thread can possibly access them
//!
//! Requirements: 12 (Lock-Free Concurrency)

const std = @import("std");
const Allocator = std.mem.Allocator;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const ObjRef = @import("objref.zig").ObjRef;
const Arena = @import("arena.zig").Arena;

// ============================================================================
// Epoch-Based Reclamation (EBR)
// ============================================================================

/// Global epoch counter
/// Threads announce their current epoch when accessing shared data
pub const EpochManager = struct {
    /// Current global epoch (atomic)
    global_epoch: std.atomic.Value(u64),
    /// Thread-local epoch records
    thread_epochs: std.ArrayList(*ThreadEpoch),
    /// Mutex for managing thread epoch list
    mutex: std.Thread.Mutex,
    /// Allocator for internal structures
    allocator: Allocator,

    pub const ThreadEpoch = struct {
        /// Thread ID
        thread_id: std.Thread.Id,
        /// Current epoch this thread is in (atomic)
        /// 0 means thread is not in a critical section
        local_epoch: std.atomic.Value(u64),
        /// Whether this record is active
        active: std.atomic.Value(bool),

        pub fn init(thread_id: std.Thread.Id) ThreadEpoch {
            return .{
                .thread_id = thread_id,
                .local_epoch = std.atomic.Value(u64).init(0),
                .active = std.atomic.Value(bool).init(true),
            };
        }

        /// Enter critical section - announce current epoch
        pub fn enter(self: *ThreadEpoch, global_epoch: u64) void {
            self.local_epoch.store(global_epoch, .seq_cst);
        }

        /// Exit critical section
        pub fn exit(self: *ThreadEpoch) void {
            self.local_epoch.store(0, .seq_cst);
        }

        /// Get current epoch (0 if not in critical section)
        pub fn getEpoch(self: *const ThreadEpoch) u64 {
            return self.local_epoch.load(.seq_cst);
        }

        /// Check if thread is in critical section
        pub fn isActive(self: *const ThreadEpoch) bool {
            return self.local_epoch.load(.seq_cst) != 0;
        }
    };

    pub fn init(allocator: Allocator) EpochManager {
        return .{
            .global_epoch = std.atomic.Value(u64).init(1),
            .thread_epochs = .{},
            .mutex = .{},
            .allocator = allocator,
        };
    }

    pub fn deinit(self: *EpochManager) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        for (self.thread_epochs.items) |epoch| {
            self.allocator.destroy(epoch);
        }
        self.thread_epochs.deinit(self.allocator);
    }

    /// Get or create thread epoch record for current thread
    pub fn getThreadEpoch(self: *EpochManager) !*ThreadEpoch {
        const thread_id = std.Thread.getCurrentId();

        self.mutex.lock();
        defer self.mutex.unlock();

        // Look for existing record
        for (self.thread_epochs.items) |epoch| {
            if (epoch.thread_id == thread_id and epoch.active.load(.seq_cst)) {
                return epoch;
            }
        }

        // Create new record
        const epoch = try self.allocator.create(ThreadEpoch);
        epoch.* = ThreadEpoch.init(thread_id);
        try self.thread_epochs.append(self.allocator, epoch);

        return epoch;
    }

    /// Advance global epoch
    pub fn advanceEpoch(self: *EpochManager) u64 {
        return self.global_epoch.fetchAdd(1, .seq_cst) + 1;
    }

    /// Get current global epoch
    pub fn getCurrentEpoch(self: *const EpochManager) u64 {
        return self.global_epoch.load(.seq_cst);
    }

    /// Get minimum epoch across all active threads
    /// Objects retired before this epoch can be safely freed
    pub fn getMinimumEpoch(self: *EpochManager) u64 {
        self.mutex.lock();
        defer self.mutex.unlock();

        var min_epoch = self.global_epoch.load(.seq_cst);

        for (self.thread_epochs.items) |epoch| {
            if (!epoch.active.load(.seq_cst)) continue;

            const local = epoch.getEpoch();
            if (local != 0 and local < min_epoch) {
                min_epoch = local;
            }
        }

        return min_epoch;
    }

    /// Enter critical section for current thread
    pub fn enter(self: *EpochManager) !*ThreadEpoch {
        const epoch = try self.getThreadEpoch();
        const global = self.getCurrentEpoch();
        epoch.enter(global);
        return epoch;
    }

    /// Exit critical section for current thread
    pub fn exit(_: *EpochManager, epoch: *ThreadEpoch) void {
        epoch.exit();
    }
};

/// Guard for automatic epoch management (RAII pattern)
pub const EpochGuard = struct {
    manager: *EpochManager,
    thread_epoch: *EpochManager.ThreadEpoch,

    pub fn init(manager: *EpochManager) !EpochGuard {
        const thread_epoch = try manager.enter();
        return .{
            .manager = manager,
            .thread_epoch = thread_epoch,
        };
    }

    pub fn deinit(self: *EpochGuard) void {
        self.manager.exit(self.thread_epoch);
    }
};

// ============================================================================
// Reference-Counted Object
// ============================================================================

/// Reference-counted object with atomic refcount
pub const RefCountedObject = struct {
    /// Object reference in arena
    objref: ObjRef,
    /// Atomic reference count
    refcount: std.atomic.Value(u32),
    /// Epoch when this object was retired (0 if not retired)
    retire_epoch: std.atomic.Value(u64),

    pub fn init(objref: ObjRef) RefCountedObject {
        return .{
            .objref = objref,
            .refcount = std.atomic.Value(u32).init(1), // Start with 1 reference
            .retire_epoch = std.atomic.Value(u64).init(0),
        };
    }

    /// Acquire a reference (increment refcount)
    /// Returns true if successful, false if object is being retired
    pub fn acquire(self: *RefCountedObject) bool {
        var current = self.refcount.load(.seq_cst);

        while (current > 0) {
            if (self.refcount.cmpxchgWeak(current, current + 1, .seq_cst, .seq_cst)) |updated| {
                current = updated;
                continue;
            }
            return true; // Successfully acquired
        }

        return false; // Object is being retired
    }

    /// Release a reference (decrement refcount)
    /// Returns true if this was the last reference
    pub fn release(self: *RefCountedObject) bool {
        const old_count = self.refcount.fetchSub(1, .seq_cst);
        return old_count == 1; // Was this the last reference?
    }

    /// Get current reference count (may be stale immediately)
    pub fn getRefCount(self: *const RefCountedObject) u32 {
        return self.refcount.load(.seq_cst);
    }

    /// Mark object as retired at given epoch
    pub fn retire(self: *RefCountedObject, epoch: u64) void {
        self.retire_epoch.store(epoch, .seq_cst);
    }

    /// Get epoch when object was retired (0 if not retired)
    pub fn getRetireEpoch(self: *const RefCountedObject) u64 {
        return self.retire_epoch.load(.seq_cst);
    }

    /// Check if object is retired
    pub fn isRetired(self: *const RefCountedObject) bool {
        return self.retire_epoch.load(.seq_cst) != 0;
    }
};

// ============================================================================
// Lock-Free Reference Counting Manager
// ============================================================================

/// Manager for lock-free reference counting with epoch-based reclamation
pub const LockFreeRefCountManager = struct {
    /// Epoch manager for safe reclamation
    epoch_manager: *EpochManager,
    /// Map from ObjRef to RefCountedObject
    objects: std.AutoHashMap(u64, *RefCountedObject),
    /// Mutex for objects map (only for map operations, not refcounting)
    objects_mutex: std.Thread.Mutex,
    /// Retired objects waiting for reclamation
    retired_objects: std.ArrayList(RetiredObject),
    /// Mutex for retired list
    retired_mutex: std.Thread.Mutex,
    /// Arena for object storage
    arena: *Arena,
    /// Allocator for internal structures
    allocator: Allocator,
    /// Statistics
    stats: Stats,

    const RetiredObject = struct {
        obj: *RefCountedObject,
        retire_epoch: u64,
    };

    pub const Stats = struct {
        objects_created: std.atomic.Value(u64),
        objects_acquired: std.atomic.Value(u64),
        objects_released: std.atomic.Value(u64),
        objects_retired: std.atomic.Value(u64),
        objects_reclaimed: std.atomic.Value(u64),
        bytes_reclaimed: std.atomic.Value(u64),

        pub fn init() Stats {
            return .{
                .objects_created = std.atomic.Value(u64).init(0),
                .objects_acquired = std.atomic.Value(u64).init(0),
                .objects_released = std.atomic.Value(u64).init(0),
                .objects_retired = std.atomic.Value(u64).init(0),
                .objects_reclaimed = std.atomic.Value(u64).init(0),
                .bytes_reclaimed = std.atomic.Value(u64).init(0),
            };
        }
    };

    pub fn init(allocator: Allocator, arena: *Arena, epoch_manager: *EpochManager) LockFreeRefCountManager {
        return .{
            .epoch_manager = epoch_manager,
            .objects = std.AutoHashMap(u64, *RefCountedObject).init(allocator),
            .objects_mutex = .{},
            .retired_objects = .{},
            .retired_mutex = .{},
            .arena = arena,
            .allocator = allocator,
            .stats = Stats.init(),
        };
    }

    pub fn deinit(self: *LockFreeRefCountManager) void {
        // Clean up all objects
        self.objects_mutex.lock();
        var it = self.objects.iterator();
        while (it.next()) |entry| {
            self.allocator.destroy(entry.value_ptr.*);
        }
        self.objects.deinit();
        self.objects_mutex.unlock();

        // Clean up retired objects
        self.retired_mutex.lock();
        for (self.retired_objects.items) |retired| {
            self.allocator.destroy(retired.obj);
        }
        self.retired_objects.deinit(self.allocator);
        self.retired_mutex.unlock();
    }

    /// Create a new reference-counted object
    pub fn createObject(self: *LockFreeRefCountManager, objref: ObjRef) !*RefCountedObject {
        const obj = try self.allocator.create(RefCountedObject);
        obj.* = RefCountedObject.init(objref);

        self.objects_mutex.lock();
        defer self.objects_mutex.unlock();

        try self.objects.put(objref.getOffset(), obj);

        _ = self.stats.objects_created.fetchAdd(1, .seq_cst);

        return obj;
    }

    /// Get reference-counted object by ObjRef
    pub fn getObject(self: *LockFreeRefCountManager, objref: ObjRef) ?*RefCountedObject {
        self.objects_mutex.lock();
        defer self.objects_mutex.unlock();

        return self.objects.get(objref.getOffset());
    }

    /// Acquire a reference to an object
    pub fn acquire(self: *LockFreeRefCountManager, objref: ObjRef) !*RefCountedObject {
        const obj = self.getObject(objref) orelse return error.ObjectNotFound;

        if (!obj.acquire()) {
            return error.ObjectRetired;
        }

        _ = self.stats.objects_acquired.fetchAdd(1, .seq_cst);

        return obj;
    }

    /// Release a reference to an object
    /// If this is the last reference, retire the object for reclamation
    pub fn release(self: *LockFreeRefCountManager, obj: *RefCountedObject) !void {
        const is_last = obj.release();

        _ = self.stats.objects_released.fetchAdd(1, .seq_cst);

        if (is_last) {
            // This was the last reference - retire the object
            try self.retireObject(obj);
        }
    }

    /// Retire an object for later reclamation
    fn retireObject(self: *LockFreeRefCountManager, obj: *RefCountedObject) !void {
        const current_epoch = self.epoch_manager.getCurrentEpoch();
        obj.retire(current_epoch);

        self.retired_mutex.lock();
        defer self.retired_mutex.unlock();

        try self.retired_objects.append(self.allocator, .{
            .obj = obj,
            .retire_epoch = current_epoch,
        });

        _ = self.stats.objects_retired.fetchAdd(1, .seq_cst);

        // Try to reclaim some objects
        self.tryReclaim();
    }

    /// Try to reclaim retired objects that are safe to free
    fn tryReclaim(self: *LockFreeRefCountManager) void {
        const min_epoch = self.epoch_manager.getMinimumEpoch();

        var i: usize = 0;
        while (i < self.retired_objects.items.len) {
            const retired = self.retired_objects.items[i];

            // Can only reclaim if retired before minimum epoch
            if (retired.retire_epoch >= min_epoch) {
                i += 1;
                continue;
            }

            // Safe to reclaim
            self.reclaimObject(retired.obj) catch {
                i += 1;
                continue;
            };

            // Remove from retired list
            _ = self.retired_objects.swapRemove(i);
            // Don't increment i since we removed an element
        }
    }

    /// Reclaim a retired object
    fn reclaimObject(self: *LockFreeRefCountManager, obj: *RefCountedObject) !void {
        // Remove from objects map
        self.objects_mutex.lock();
        _ = self.objects.remove(obj.objref.getOffset());
        self.objects_mutex.unlock();

        // Get object size for stats
        const header_bytes = try self.arena.readAt(obj.objref.getOffset(), ObjectHeader.headerSize());
        const header = try ObjectHeader.fromBytes(header_bytes);

        // Mark object as deleted in arena
        try self.arena.delete(obj.objref);

        // Update stats
        _ = self.stats.objects_reclaimed.fetchAdd(1, .seq_cst);
        _ = self.stats.bytes_reclaimed.fetchAdd(header.size, .seq_cst);

        // Free the RefCountedObject structure
        self.allocator.destroy(obj);
    }

    /// Force reclamation of all eligible retired objects
    pub fn forceReclaim(self: *LockFreeRefCountManager) void {
        self.retired_mutex.lock();
        defer self.retired_mutex.unlock();

        self.tryReclaim();
    }

    /// Advance epoch and try to reclaim objects
    pub fn advanceEpochAndReclaim(self: *LockFreeRefCountManager) void {
        _ = self.epoch_manager.advanceEpoch();

        self.retired_mutex.lock();
        defer self.retired_mutex.unlock();

        self.tryReclaim();
    }

    /// Get statistics
    pub fn getStats(self: *const LockFreeRefCountManager) Stats {
        return self.stats;
    }
};

// ============================================================================
// Convenience Functions
// ============================================================================

/// RAII guard for automatic reference release
pub const RefGuard = struct {
    manager: *LockFreeRefCountManager,
    obj: *RefCountedObject,

    pub fn init(manager: *LockFreeRefCountManager, obj: *RefCountedObject) RefGuard {
        return .{
            .manager = manager,
            .obj = obj,
        };
    }

    pub fn deinit(self: *RefGuard) void {
        self.manager.release(self.obj) catch {};
    }

    pub fn getObjRef(self: *const RefGuard) ObjRef {
        return self.obj.objref;
    }
};

// ============================================================================
// Tests
// ============================================================================

test "EpochManager: basic operations" {
    const allocator = std.testing.allocator;
    var manager = EpochManager.init(allocator);
    defer manager.deinit();

    const initial_epoch = manager.getCurrentEpoch();
    try std.testing.expectEqual(@as(u64, 1), initial_epoch);

    // Advance epoch
    const new_epoch = manager.advanceEpoch();
    try std.testing.expectEqual(@as(u64, 2), new_epoch);
    try std.testing.expectEqual(@as(u64, 2), manager.getCurrentEpoch());
}

test "EpochManager: thread epoch" {
    const allocator = std.testing.allocator;
    var manager = EpochManager.init(allocator);
    defer manager.deinit();

    const thread_epoch = try manager.getThreadEpoch();
    try std.testing.expectEqual(std.Thread.getCurrentId(), thread_epoch.thread_id);

    // Initially not in critical section
    try std.testing.expectEqual(@as(u64, 0), thread_epoch.getEpoch());
    try std.testing.expect(!thread_epoch.isActive());

    // Enter critical section
    thread_epoch.enter(manager.getCurrentEpoch());
    try std.testing.expect(thread_epoch.isActive());

    // Exit critical section
    thread_epoch.exit();
    try std.testing.expect(!thread_epoch.isActive());
}

test "EpochManager: minimum epoch" {
    const allocator = std.testing.allocator;
    var manager = EpochManager.init(allocator);
    defer manager.deinit();

    // No active threads - minimum should be current
    const min1 = manager.getMinimumEpoch();
    try std.testing.expectEqual(manager.getCurrentEpoch(), min1);

    // Enter critical section
    const thread_epoch = try manager.enter();
    defer manager.exit(thread_epoch);

    // Advance global epoch
    _ = manager.advanceEpoch();

    // Minimum should still be the thread's epoch
    const min2 = manager.getMinimumEpoch();
    try std.testing.expect(min2 < manager.getCurrentEpoch());
}

test "EpochGuard: RAII pattern" {
    const allocator = std.testing.allocator;
    var manager = EpochManager.init(allocator);
    defer manager.deinit();

    {
        var guard = try EpochGuard.init(&manager);
        defer guard.deinit();

        // Should be in critical section
        try std.testing.expect(guard.thread_epoch.isActive());
    }

    // Should have exited critical section
    const thread_epoch = try manager.getThreadEpoch();
    try std.testing.expect(!thread_epoch.isActive());
}

test "RefCountedObject: basic operations" {
    const objref = ObjRef.init(1000);
    var obj = RefCountedObject.init(objref);

    try std.testing.expectEqual(@as(u32, 1), obj.getRefCount());
    try std.testing.expect(!obj.isRetired());

    // Acquire references
    try std.testing.expect(obj.acquire());
    try std.testing.expectEqual(@as(u32, 2), obj.getRefCount());

    try std.testing.expect(obj.acquire());
    try std.testing.expectEqual(@as(u32, 3), obj.getRefCount());

    // Release references
    try std.testing.expect(!obj.release()); // Not last
    try std.testing.expectEqual(@as(u32, 2), obj.getRefCount());

    try std.testing.expect(!obj.release()); // Not last
    try std.testing.expectEqual(@as(u32, 1), obj.getRefCount());

    try std.testing.expect(obj.release()); // Last reference
    try std.testing.expectEqual(@as(u32, 0), obj.getRefCount());
}

test "RefCountedObject: retire" {
    const objref = ObjRef.init(1000);
    var obj = RefCountedObject.init(objref);

    try std.testing.expect(!obj.isRetired());

    obj.retire(42);
    try std.testing.expect(obj.isRetired());
    try std.testing.expectEqual(@as(u64, 42), obj.getRetireEpoch());
}

test "RefCountedObject: cannot acquire retired" {
    const objref = ObjRef.init(1000);
    var obj = RefCountedObject.init(objref);

    // Release last reference
    try std.testing.expect(obj.release());

    // Cannot acquire after refcount reaches 0
    try std.testing.expect(!obj.acquire());
}
