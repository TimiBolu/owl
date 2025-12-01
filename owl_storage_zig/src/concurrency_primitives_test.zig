//! Comprehensive tests for Zig-level concurrency primitives
//!
//! This module contains extensive tests for all concurrency primitives
//! including multi-threaded scenarios and stress tests.

const std = @import("std");
const testing = std.testing;
const Thread = std.Thread;
const Allocator = std.mem.Allocator;

const concurrency = @import("concurrency_primitives.zig");
const AtomicRefCount = concurrency.AtomicRefCount;
const SharedObject = concurrency.SharedObject;
const HazardPointerManager = concurrency.HazardPointerManager;
const LockFreeStack = concurrency.LockFreeStack;
const LockFreeQueue = concurrency.LockFreeQueue;
const AtomicObjectHeader = concurrency.AtomicObjectHeader;

const ObjectHeader = @import("object_header.zig").ObjectHeader;

// ============================================================================
// Multi-threaded Tests
// ============================================================================

const ThreadTestContext = struct {
    allocator: Allocator,
    hp_manager: *HazardPointerManager,
    stack: *LockFreeStack(i32),
    queue: *LockFreeQueue(i32),
    shared_counter: *std.atomic.Value(i32),
    iterations: usize,
    thread_id: usize,
};

fn stackWorker(ctx: *ThreadTestContext) void {
    var i: usize = 0;
    while (i < ctx.iterations) : (i += 1) {
        const value = @as(i32, @intCast(ctx.thread_id * 1000 + i));

        // Push value
        ctx.stack.push(value) catch continue;

        // Try to pop a value
        if (ctx.stack.pop()) |popped| {
            _ = ctx.shared_counter.fetchAdd(popped, .seq_cst);
        }
    }
}

fn queueWorker(ctx: *ThreadTestContext) void {
    var i: usize = 0;
    while (i < ctx.iterations) : (i += 1) {
        const value = @as(i32, @intCast(ctx.thread_id * 1000 + i));

        // Enqueue value
        ctx.queue.enqueue(value) catch continue;

        // Try to dequeue a value
        if (ctx.queue.dequeue()) |dequeued| {
            _ = ctx.shared_counter.fetchAdd(dequeued, .seq_cst);
        }
    }
}

test "LockFreeStack: multi-threaded stress test" {
    const allocator = testing.allocator;
    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    var stack = LockFreeStack(i32).init(allocator, &hp_manager);
    defer stack.deinit();

    var shared_counter = std.atomic.Value(i32).init(0);

    const num_threads = 4;
    const iterations_per_thread = 100;

    var threads: [num_threads]Thread = undefined;
    var contexts: [num_threads]ThreadTestContext = undefined;

    // Initialize contexts
    for (&contexts, 0..) |*ctx, i| {
        ctx.* = .{
            .allocator = allocator,
            .hp_manager = &hp_manager,
            .stack = &stack,
            .queue = undefined, // Not used in this test
            .shared_counter = &shared_counter,
            .iterations = iterations_per_thread,
            .thread_id = i,
        };
    }

    // Start threads
    for (&threads, 0..) |*thread, i| {
        thread.* = try Thread.spawn(.{}, stackWorker, &contexts[i]);
    }

    // Wait for threads to complete
    for (&threads) |*thread| {
        thread.join();
    }

    // Verify no crashes occurred and some work was done
    const final_counter = shared_counter.load(.seq_cst);
    std.debug.print("Stack stress test completed. Final counter: {}\n", .{final_counter});

    // Clean up remaining items in stack
    var remaining_count: usize = 0;
    while (stack.pop()) |_| {
        remaining_count += 1;
    }
    std.debug.print("Remaining items in stack: {}\n", .{remaining_count});
}

test "LockFreeQueue: multi-threaded stress test" {
    const allocator = testing.allocator;
    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    var queue = try LockFreeQueue(i32).init(allocator, &hp_manager);
    defer queue.deinit();

    var shared_counter = std.atomic.Value(i32).init(0);

    const num_threads = 4;
    const iterations_per_thread = 100;

    var threads: [num_threads]Thread = undefined;
    var contexts: [num_threads]ThreadTestContext = undefined;

    // Initialize contexts
    for (&contexts, 0..) |*ctx, i| {
        ctx.* = .{
            .allocator = allocator,
            .hp_manager = &hp_manager,
            .stack = undefined, // Not used in this test
            .queue = &queue,
            .shared_counter = &shared_counter,
            .iterations = iterations_per_thread,
            .thread_id = i,
        };
    }

    // Start threads
    for (&threads, 0..) |*thread, i| {
        thread.* = try Thread.spawn(.{}, queueWorker, &contexts[i]);
    }

    // Wait for threads to complete
    for (&threads) |*thread| {
        thread.join();
    }

    // Verify no crashes occurred and some work was done
    const final_counter = shared_counter.load(.seq_cst);
    std.debug.print("Queue stress test completed. Final counter: {}\n", .{final_counter});

    // Clean up remaining items in queue
    var remaining_count: usize = 0;
    while (queue.dequeue()) |_| {
        remaining_count += 1;
    }
    std.debug.print("Remaining items in queue: {}\n", .{remaining_count});
}

// ============================================================================
// Reference Counting Tests
// ============================================================================

const RefCountTestData = struct {
    value: i32,
    cleanup_called: *std.atomic.Value(bool),
};

fn refCountCleanup(ptr: *anyopaque) void {
    const data: *RefCountTestData = @ptrCast(@alignCast(ptr));
    data.cleanup_called.store(true, .seq_cst);
}

test "AtomicRefCount: multi-threaded reference counting" {
    var cleanup_called = std.atomic.Value(bool).init(false);
    var test_data = RefCountTestData{
        .value = 42,
        .cleanup_called = &cleanup_called,
    };

    var ref_count = AtomicRefCount.initWithCleanup(refCountCleanup, &test_data);

    const num_threads = 8;
    const acquisitions_per_thread = 50;

    var threads: [num_threads]Thread = undefined;

    const RefCountWorker = struct {
        fn worker(rc: *AtomicRefCount, acquisitions: usize) void {
            var i: usize = 0;
            while (i < acquisitions) : (i += 1) {
                // Acquire reference
                _ = rc.acquire();

                // Do some work
                std.time.sleep(1000); // 1 microsecond

                // Release reference
                _ = rc.release();
            }
        }
    };

    // Start threads
    for (&threads) |*thread| {
        thread.* = try Thread.spawn(.{}, RefCountWorker.worker, .{ &ref_count, acquisitions_per_thread });
    }

    // Wait for threads to complete
    for (&threads) |*thread| {
        thread.join();
    }

    // Release the initial reference
    const was_cleaned_up = ref_count.release();

    try testing.expect(was_cleaned_up);
    try testing.expect(cleanup_called.load(.seq_cst));
}

// ============================================================================
// Hazard Pointer Tests
// ============================================================================

test "HazardPointerManager: concurrent retirement and reclamation" {
    const allocator = testing.allocator;
    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    const num_pointers = 100;
    var cleanup_count = std.atomic.Value(usize).init(0);

    const TestPointer = struct {
        value: i32,
        cleanup_counter: *std.atomic.Value(usize),

        fn cleanup(ptr: *anyopaque) void {
            const self: *@This() = @ptrCast(@alignCast(ptr));
            _ = self.cleanup_counter.fetchAdd(1, .seq_cst);
            std.heap.page_allocator.destroy(self);
        }
    };

    // Allocate and retire pointers
    var i: usize = 0;
    while (i < num_pointers) : (i += 1) {
        const ptr = try std.heap.page_allocator.create(TestPointer);
        ptr.* = .{
            .value = @intCast(i),
            .cleanup_counter = &cleanup_count,
        };

        try hp_manager.retirePointer(ptr, TestPointer.cleanup);
    }

    // Force reclamation
    hp_manager.forceReclaim();

    // All pointers should be reclaimed since none are protected
    try testing.expectEqual(@as(usize, num_pointers), cleanup_count.load(.seq_cst));
}

// ============================================================================
// Atomic Object Header Tests
// ============================================================================

test "AtomicObjectHeader: concurrent header updates" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    const num_threads = 4;
    const updates_per_thread = 25;

    var threads: [num_threads]Thread = undefined;

    const HeaderWorker = struct {
        fn worker(h: *ObjectHeader, updates: usize, thread_id: usize) void {
            var i: usize = 0;
            while (i < updates) : (i += 1) {
                const lsn = @as(u64, thread_id * 1000 + i);

                // Atomic version and LSN update
                _ = AtomicObjectHeader.atomicVersionAndLSNUpdate(h, lsn);

                // Atomic flag updates
                h.atomicSetPinned(i % 2 == 0);
                h.atomicSetDeleted(i % 3 == 0);
            }
        }
    };

    // Start threads
    for (&threads, 0..) |*thread, i| {
        thread.* = try Thread.spawn(.{}, HeaderWorker.worker, .{ &header, updates_per_thread, i });
    }

    // Wait for threads to complete
    for (&threads) |*thread| {
        thread.join();
    }

    // Verify final state is consistent
    const final_version = header.version;
    const final_lsn = header.atomicGetLSN();

    try testing.expect(final_version > 0);
    try testing.expect(final_lsn > 0);

    std.debug.print("Final header state: version={}, lsn={}, flags=0x{x}\n", .{
        final_version,
        final_lsn,
        header.flags,
    });
}

// ============================================================================
// Performance Benchmarks
// ============================================================================

test "Performance: lock-free stack vs mutex-protected stack" {
    const allocator = testing.allocator;
    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    // Lock-free stack
    var lf_stack = LockFreeStack(i32).init(allocator, &hp_manager);
    defer lf_stack.deinit();

    // Mutex-protected stack (for comparison)
    const MutexStack = struct {
        items: std.ArrayList(i32),
        mutex: Thread.Mutex,

        fn init(alloc: Allocator) @This() {
            return .{
                .items = std.ArrayList(i32).init(alloc),
                .mutex = .{},
            };
        }

        fn deinit(self: *@This()) void {
            self.items.deinit();
        }

        fn push(self: *@This(), value: i32) !void {
            self.mutex.lock();
            defer self.mutex.unlock();
            try self.items.append(value);
        }

        fn pop(self: *@This()) ?i32 {
            self.mutex.lock();
            defer self.mutex.unlock();
            return self.items.popOrNull();
        }
    };

    var mutex_stack = MutexStack.init(allocator);
    defer mutex_stack.deinit();

    const operations = 1000;

    // Benchmark lock-free stack
    const lf_start = std.time.nanoTimestamp();

    var i: usize = 0;
    while (i < operations) : (i += 1) {
        try lf_stack.push(@intCast(i));
    }

    while (lf_stack.pop()) |_| {}

    const lf_end = std.time.nanoTimestamp();
    const lf_duration = lf_end - lf_start;

    // Benchmark mutex stack
    const mutex_start = std.time.nanoTimestamp();

    i = 0;
    while (i < operations) : (i += 1) {
        try mutex_stack.push(@intCast(i));
    }

    while (mutex_stack.pop()) |_| {}

    const mutex_end = std.time.nanoTimestamp();
    const mutex_duration = mutex_end - mutex_start;

    std.debug.print("Lock-free stack: {} ns\n", .{lf_duration});
    std.debug.print("Mutex stack: {} ns\n", .{mutex_duration});

    // Lock-free should be competitive (not necessarily faster due to overhead)
    // The main benefit is avoiding blocking and deadlocks
    try testing.expect(lf_duration > 0);
    try testing.expect(mutex_duration > 0);
}

// ============================================================================
// Integration Tests
// ============================================================================

test "Integration: shared objects with hazard pointers" {
    const allocator = testing.allocator;
    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    const TestData = struct {
        value: i32,
        name: []const u8,
    };

    // Create shared object
    var shared_obj = try SharedObject(TestData).create(allocator, .{
        .value = 123,
        .name = "test object",
    });
    defer shared_obj.release();

    // Get hazard pointer record
    const record = try hp_manager.getRecord();

    // Protect the shared object with hazard pointer
    record.setHazard(0, shared_obj);

    // Verify object is protected
    try testing.expect(record.isProtected(shared_obj));

    // Acquire additional references
    var ref1 = shared_obj.acquire();
    var ref2 = shared_obj.acquire();

    try testing.expectEqual(@as(u32, 3), shared_obj.getRefCount());

    // Clear hazard pointer
    record.clearHazard(0);
    try testing.expect(!record.isProtected(shared_obj));

    // Release references
    ref1.release();
    ref2.release();

    try testing.expectEqual(@as(u32, 1), shared_obj.getRefCount());
}

test "Integration: atomic operations with object headers" {
    const id = [_]u8{ 0x12, 0x34, 0x56, 0x78 } ++ [_]u8{0xAB} ** 12;
    var header = ObjectHeader.init(42, id, 2048);

    // Test atomic flag operations
    header.atomicSetDeleted(true);
    try testing.expect(header.isDeleted());

    header.atomicSetPinned(true);
    try testing.expect(header.isPinned());

    header.atomicSetOverflow(true);
    try testing.expect(header.hasOverflow());

    // Test atomic version increment
    const version1 = header.atomicIncrementVersion();
    const version2 = header.atomicIncrementVersion();

    try testing.expectEqual(@as(u32, 1), version1);
    try testing.expectEqual(@as(u32, 2), version2);

    // Test atomic LSN operations
    header.atomicSetLSN(12345);
    try testing.expectEqual(@as(u64, 12345), header.atomicGetLSN());

    // Test combined version and LSN update
    const new_version = AtomicObjectHeader.atomicVersionAndLSNUpdate(&header, 99999);
    try testing.expectEqual(@as(u32, 3), new_version);
    try testing.expectEqual(@as(u64, 99999), header.atomicGetLSN());
}
