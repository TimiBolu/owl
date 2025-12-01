// Lock-free stack implementation using atomic operations
// Based on Treiber's stack algorithm

const std = @import("std");

/// Lock-free stack node
pub fn Node(comptime T: type) type {
    return struct {
        value: T,
        next: ?*@This(),
    };
}

/// Lock-free stack using Treiber's algorithm
pub fn LockFreeStack(comptime T: type) type {
    return struct {
        const Self = @This();
        const NodeType = Node(T);

        head: std.atomic.Value(?*NodeType),
        allocator: std.mem.Allocator,
        count: std.atomic.Value(usize),

        pub fn init(allocator: std.mem.Allocator) Self {
            return .{
                .head = std.atomic.Value(?*NodeType).init(null),
                .allocator = allocator,
                .count = std.atomic.Value(usize).init(0),
            };
        }

        pub fn deinit(self: *Self) void {
            // Pop all remaining nodes and free them
            while (self.pop()) |_| {}
        }

        /// Push a value onto the stack (thread-safe)
        pub fn push(self: *Self, value: T) !void {
            const node = try self.allocator.create(NodeType);
            node.value = value;

            // Treiber's algorithm: CAS loop
            while (true) {
                const current_head = self.head.load(.acquire);
                node.next = current_head;

                if (self.head.cmpxchgWeak(
                    current_head,
                    node,
                    .release,
                    .acquire,
                )) |_| {
                    // CAS failed, retry
                    continue;
                } else {
                    // CAS succeeded
                    _ = self.count.fetchAdd(1, .monotonic);
                    return;
                }
            }
        }

        /// Pop a value from the stack (thread-safe)
        pub fn pop(self: *Self) ?T {
            while (true) {
                const current_head = self.head.load(.acquire);
                if (current_head == null) {
                    return null;
                }

                const next = current_head.?.next;

                if (self.head.cmpxchgWeak(
                    current_head,
                    next,
                    .release,
                    .acquire,
                )) |_| {
                    // CAS failed, retry
                    continue;
                } else {
                    // CAS succeeded
                    _ = self.count.fetchSub(1, .monotonic);
                    const value = current_head.?.value;
                    self.allocator.destroy(current_head.?);
                    return value;
                }
            }
        }

        /// Try to pop without blocking (returns null if empty)
        pub fn tryPop(self: *Self) ?T {
            return self.pop();
        }

        /// Check if stack is empty
        pub fn isEmpty(self: *const Self) bool {
            return self.head.load(.acquire) == null;
        }

        /// Get approximate count (may be stale in concurrent scenarios)
        pub fn len(self: *const Self) usize {
            return self.count.load(.monotonic);
        }
    };
}

// Tests
test "LockFreeStack: basic push and pop" {
    const allocator = std.testing.allocator;
    var stack = LockFreeStack(u32).init(allocator);
    defer stack.deinit();

    try stack.push(1);
    try stack.push(2);
    try stack.push(3);

    try std.testing.expectEqual(@as(usize, 3), stack.len());

    try std.testing.expectEqual(@as(u32, 3), stack.pop().?);
    try std.testing.expectEqual(@as(u32, 2), stack.pop().?);
    try std.testing.expectEqual(@as(u32, 1), stack.pop().?);
    try std.testing.expect(stack.pop() == null);
}

test "LockFreeStack: empty stack" {
    const allocator = std.testing.allocator;
    var stack = LockFreeStack(u32).init(allocator);
    defer stack.deinit();

    try std.testing.expect(stack.isEmpty());
    try std.testing.expect(stack.pop() == null);
}

test "LockFreeStack: LIFO order" {
    const allocator = std.testing.allocator;
    var stack = LockFreeStack(u32).init(allocator);
    defer stack.deinit();

    try stack.push(10);
    try stack.push(20);
    try stack.push(30);

    // Should pop in reverse order (LIFO)
    try std.testing.expectEqual(@as(u32, 30), stack.pop().?);
    try std.testing.expectEqual(@as(u32, 20), stack.pop().?);
    try std.testing.expectEqual(@as(u32, 10), stack.pop().?);
}
