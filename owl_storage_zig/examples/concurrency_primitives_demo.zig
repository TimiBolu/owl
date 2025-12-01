//! Demonstration of Zig-level concurrency primitives
//!
//! This example shows how to use the various concurrency primitives
//! implemented for the Owl storage engine, including:
//! - Atomic reference counting for shared objects
//! - Hazard pointers for safe memory reclamation
//! - Lock-free data structures (stack and queue)
//! - Atomic operations on object headers

const std = @import("std");
const print = std.debug.print;
const Thread = std.Thread;
const Allocator = std.mem.Allocator;

// Import our concurrency primitives
const concurrency = @import("../src/concurrency_primitives.zig");
const AtomicRefCount = concurrency.AtomicRefCount;
const SharedObject = concurrency.SharedObject;
const HazardPointerManager = concurrency.HazardPointerManager;
const LockFreeStack = concurrency.LockFreeStack;
const LockFreeQueue = concurrency.LockFreeQueue;
const AtomicObjectHeader = concurrency.AtomicObjectHeader;
const ObjectHeader = @import("../src/object_header.zig").ObjectHeader;

// ============================================================================
// Example 1: Atomic Reference Counting
// ============================================================================

const DatabaseConnection = struct {
    id: u32,
    host: []const u8,
    port: u16,
    is_connected: bool,

    fn connect(self: *DatabaseConnection) void {
        print("Connecting to database {}:{}:{}\n", .{ self.id, self.host, self.port });
        self.is_connected = true;
    }

    fn disconnect(self: *DatabaseConnection) void {
        if (self.is_connected) {
            print("Disconnecting from database {}\n", .{self.id});
            self.is_connected = false;
        }
    }
};

fn demonstrateAtomicRefCounting(allocator: Allocator) !void {
    print("\n=== Atomic Reference Counting Demo ===\n");

    // Create a shared database connection
    var shared_conn = try SharedObject(DatabaseConnection).create(allocator, .{
        .id = 1,
        .host = "localhost",
        .port = 5432,
        .is_connected = false,
    });
    defer shared_conn.release();

    shared_conn.data.connect();
    print("Initial ref count: {}\n", .{shared_conn.getRefCount()});

    // Simulate multiple components using the connection
    var conn_ref1 = shared_conn.acquire();
    var conn_ref2 = shared_conn.acquire();
    var conn_ref3 = shared_conn.acquire();

    print("After acquiring 3 more references: {}\n", .{shared_conn.getRefCount()});

    // Components finish using the connection
    conn_ref1.release();
    print("After releasing 1 reference: {}\n", .{shared_conn.getRefCount()});

    conn_ref2.release();
    conn_ref3.release();
    print("After releasing remaining references: {}\n", .{shared_conn.getRefCount()});

    shared_conn.data.disconnect();
}

// ============================================================================
// Example 2: Lock-Free Data Structures
// ============================================================================

const WorkItem = struct {
    id: u32,
    priority: u8,
    data: []const u8,
};

fn demonstrateLockFreeStructures(allocator: Allocator) !void {
    print("\n=== Lock-Free Data Structures Demo ===\n");

    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    // Lock-free stack for high-priority work items
    var priority_stack = LockFreeStack(WorkItem).init(allocator, &hp_manager);
    defer priority_stack.deinit();

    // Lock-free queue for regular work items
    var work_queue = try LockFreeQueue(WorkItem).init(allocator, &hp_manager);
    defer work_queue.deinit();

    print("Adding work items to stack and queue...\n");

    // Add high-priority items to stack
    try priority_stack.push(.{ .id = 1, .priority = 10, .data = "Critical task" });
    try priority_stack.push(.{ .id = 2, .priority = 9, .data = "Important task" });
    try priority_stack.push(.{ .id = 3, .priority = 8, .data = "Urgent task" });

    // Add regular items to queue
    try work_queue.enqueue(.{ .id = 101, .priority = 5, .data = "Regular task 1" });
    try work_queue.enqueue(.{ .id = 102, .priority = 4, .data = "Regular task 2" });
    try work_queue.enqueue(.{ .id = 103, .priority = 3, .data = "Regular task 3" });

    print("\nProcessing high-priority items (LIFO from stack):\n");
    while (priority_stack.pop()) |item| {
        print("  Processing: ID={}, Priority={}, Data='{}'\n", .{ item.id, item.priority, item.data });
    }

    print("\nProcessing regular items (FIFO from queue):\n");
    while (work_queue.dequeue()) |item| {
        print("  Processing: ID={}, Priority={}, Data='{}'\n", .{ item.id, item.priority, item.data });
    }
}

// ============================================================================
// Example 3: Multi-threaded Producer-Consumer
// ============================================================================

const ProducerConsumerContext = struct {
    queue: *LockFreeQueue(i32),
    hp_manager: *HazardPointerManager,
    items_to_produce: u32,
    items_consumed: *std.atomic.Value(u32),
    stop_flag: *std.atomic.Value(bool),
};

fn producer(ctx: *ProducerConsumerContext) void {
    print("Producer starting...\n");

    var i: u32 = 0;
    while (i < ctx.items_to_produce) : (i += 1) {
        ctx.queue.enqueue(@intCast(i)) catch {
            print("Producer: Failed to enqueue item {}\n", .{i});
            continue;
        };

        // Simulate some work
        std.time.sleep(1000000); // 1ms
    }

    print("Producer finished producing {} items\n", .{ctx.items_to_produce});
}

fn consumer(ctx: *ProducerConsumerContext, consumer_id: u32) void {
    print("Consumer {} starting...\n", .{consumer_id});

    var items_processed: u32 = 0;

    while (!ctx.stop_flag.load(.seq_cst)) {
        if (ctx.queue.dequeue()) |item| {
            items_processed += 1;
            _ = ctx.items_consumed.fetchAdd(1, .seq_cst);

            if (items_processed % 10 == 0) {
                print("Consumer {}: Processed {} items (latest: {})\n", .{ consumer_id, items_processed, item });
            }
        } else {
            // No items available, sleep briefly
            std.time.sleep(100000); // 0.1ms
        }
    }

    print("Consumer {} finished, processed {} items\n", .{ consumer_id, items_processed });
}

fn demonstrateProducerConsumer(allocator: Allocator) !void {
    print("\n=== Multi-threaded Producer-Consumer Demo ===\n");

    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    var queue = try LockFreeQueue(i32).init(allocator, &hp_manager);
    defer queue.deinit();

    var items_consumed = std.atomic.Value(u32).init(0);
    var stop_flag = std.atomic.Value(bool).init(false);

    const items_to_produce = 100;
    const num_consumers = 3;

    var ctx = ProducerConsumerContext{
        .queue = &queue,
        .hp_manager = &hp_manager,
        .items_to_produce = items_to_produce,
        .items_consumed = &items_consumed,
        .stop_flag = &stop_flag,
    };

    // Start producer thread
    var producer_thread = try Thread.spawn(.{}, producer, &ctx);

    // Start consumer threads
    var consumer_threads: [num_consumers]Thread = undefined;
    for (&consumer_threads, 0..) |*thread, i| {
        thread.* = try Thread.spawn(.{}, consumer, .{ &ctx, @intCast(i) });
    }

    // Wait for producer to finish
    producer_thread.join();

    // Wait a bit for consumers to process remaining items
    std.time.sleep(100000000); // 100ms

    // Stop consumers
    stop_flag.store(true, .seq_cst);

    // Wait for consumers to finish
    for (&consumer_threads) |*thread| {
        thread.join();
    }

    const total_consumed = items_consumed.load(.seq_cst);
    print("Total items consumed: {} / {}\n", .{ total_consumed, items_to_produce });

    // Process any remaining items
    var remaining: u32 = 0;
    while (queue.dequeue()) |_| {
        remaining += 1;
    }
    print("Remaining items in queue: {}\n", .{remaining});
}

// ============================================================================
// Example 4: Atomic Object Header Operations
// ============================================================================

fn demonstrateAtomicObjectHeaders(_: Allocator) !void {
    print("\n=== Atomic Object Header Operations Demo ===\n");

    const id = [_]u8{ 0x12, 0x34, 0x56, 0x78 } ++ [_]u8{0xAB} ** 12;
    var header = ObjectHeader.init(42, id, 2048);

    print("Initial header state:\n");
    print("  Type: {}, Version: {}, Size: {}, LSN: {}\n", .{
        header.type_tag,
        header.version,
        header.size,
        header.lsn,
    });

    // Simulate concurrent updates to the header
    const num_updates = 10;
    var i: usize = 0;
    while (i < num_updates) : (i += 1) {
        // Atomic version increment and LSN update
        const new_version = AtomicObjectHeader.atomicVersionAndLSNUpdate(&header, @intCast(1000 + i));

        // Atomic flag updates
        header.atomicSetPinned(i % 2 == 0);
        header.atomicSetDeleted(i % 3 == 0);
        header.atomicSetOverflow(i % 4 == 0);

        print("Update {}: Version={}, LSN={}, Flags=0x{x:0>2}\n", .{
            i + 1,
            new_version,
            header.atomicGetLSN(),
            header.flags,
        });
    }

    // Demonstrate atomic field updates
    print("\nAtomic field updates:\n");
    AtomicObjectHeader.atomicUpdateFields(&header, 100, 9999, 0x07);
    print("After atomic update: Version={}, LSN={}, Flags=0x{x:0>2}\n", .{
        header.version,
        header.atomicGetLSN(),
        header.flags,
    });

    // Show flag interpretations
    print("\nFlag interpretations:\n");
    print("  Deleted: {}\n", .{header.isDeleted()});
    print("  Pinned: {}\n", .{header.isPinned()});
    print("  Has overflow: {}\n", .{header.hasOverflow()});
}

// ============================================================================
// Example 5: Hazard Pointer Protection
// ============================================================================

const SharedResource = struct {
    id: u32,
    data: [1024]u8,
    ref_count: AtomicRefCount,

    fn init(id: u32) SharedResource {
        return .{
            .id = id,
            .data = [_]u8{0} ** 1024,
            .ref_count = AtomicRefCount.init(),
        };
    }

    fn cleanup(ptr: *anyopaque) void {
        const resource: *SharedResource = @ptrCast(@alignCast(ptr));
        print("Cleaning up shared resource {}\n", .{resource.id});
        std.heap.page_allocator.destroy(resource);
    }
};

fn demonstrateHazardPointers(allocator: Allocator) !void {
    print("\n=== Hazard Pointer Protection Demo ===\n");

    var hp_manager = HazardPointerManager.init(allocator);
    defer hp_manager.deinit();

    // Create some shared resources
    var resources: [5]*SharedResource = undefined;
    for (&resources, 0..) |*resource, i| {
        resource.* = try std.heap.page_allocator.create(SharedResource);
        resource.*.* = SharedResource.init(@intCast(i));
    }

    print("Created {} shared resources\n", .{resources.len});

    // Get hazard pointer record for this thread
    const record = try hp_manager.getRecord();

    // Protect some resources with hazard pointers
    record.setHazard(0, resources[0]);
    record.setHazard(1, resources[2]);
    record.setHazard(2, resources[4]);

    print("Protected resources 0, 2, and 4 with hazard pointers\n");

    // Retire all resources
    for (resources) |resource| {
        try hp_manager.retirePointer(resource, SharedResource.cleanup);
    }

    print("Retired all resources\n");

    // Force reclamation - only unprotected resources should be cleaned up
    hp_manager.forceReclaim();
    print("Forced reclamation - unprotected resources should be cleaned up\n");

    // Clear hazard pointers
    record.clearHazard(0);
    record.clearHazard(1);
    record.clearHazard(2);

    print("Cleared hazard pointers\n");

    // Force reclamation again - now all resources should be cleaned up
    hp_manager.forceReclaim();
    print("Final reclamation - all resources should now be cleaned up\n");
}

// ============================================================================
// Main Demo Function
// ============================================================================

pub fn main() !void {
    var gpa = std.heap.GeneralPurposeAllocator(.{}){};
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    print("Owl Storage Engine - Concurrency Primitives Demo\n");
    print("================================================\n");

    try demonstrateAtomicRefCounting(allocator);
    try demonstrateLockFreeStructures(allocator);
    try demonstrateProducerConsumer(allocator);
    try demonstrateAtomicObjectHeaders(allocator);
    try demonstrateHazardPointers(allocator);

    print("\n=== Demo Complete ===\n");
    print("All concurrency primitives demonstrated successfully!\n");
}
