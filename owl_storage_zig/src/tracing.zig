// Distributed Tracing Support for Zig Storage Layer
// Provides trace context propagation across Rust-Zig boundary

const std = @import("std");

/// Trace context for distributed tracing
pub const TraceContext = struct {
    trace_id: [16]u8, // 128-bit trace ID
    span_id: [8]u8, // 64-bit span ID
    parent_span_id: [8]u8, // 64-bit parent span ID
    flags: u8, // Trace flags (sampled, etc.)

    const Self = @This();

    /// Create a new root trace context
    pub fn root() Self {
        var ctx = Self{
            .trace_id = undefined,
            .span_id = undefined,
            .parent_span_id = [_]u8{0} ** 8,
            .flags = 1, // Sampled
        };

        // Generate random trace and span IDs using timestamp
        const timestamp: u64 = @intCast(std.time.nanoTimestamp());
        var i: usize = 0;
        while (i < 16) : (i += 1) {
            ctx.trace_id[i] = @truncate((timestamp >> @intCast(i * 4)) ^ @as(u64, i));
        }
        i = 0;
        while (i < 8) : (i += 1) {
            ctx.span_id[i] = @truncate((timestamp >> @intCast(i * 8)) ^ @as(u64, i + 16));
        }

        return ctx;
    }

    /// Create a child span
    pub fn child(self: *const Self) Self {
        var ctx = Self{
            .trace_id = self.trace_id,
            .span_id = undefined,
            .parent_span_id = self.span_id,
            .flags = self.flags,
        };

        // Generate new span ID using timestamp
        const timestamp: u64 = @intCast(std.time.nanoTimestamp());
        var i: usize = 0;
        while (i < 8) : (i += 1) {
            ctx.span_id[i] = @truncate((timestamp >> @intCast(i * 8)) ^ @as(u64, i + 32));
        }

        return ctx;
    }

    /// Check if trace is sampled
    pub fn isSampled(self: *const Self) bool {
        return (self.flags & 1) != 0;
    }

    /// Convert trace ID to hex string
    pub fn traceIdHex(self: *const Self, buf: []u8) ![]const u8 {
        if (buf.len < 32) return error.BufferTooSmall;
        return std.fmt.bufPrint(buf, "{x:0>32}", .{std.fmt.fmtSliceHexLower(&self.trace_id)}) catch unreachable;
    }

    /// Convert span ID to hex string
    pub fn spanIdHex(self: *const Self, buf: []u8) ![]const u8 {
        if (buf.len < 16) return error.BufferTooSmall;
        return std.fmt.bufPrint(buf, "{x:0>16}", .{std.fmt.fmtSliceHexLower(&self.span_id)}) catch unreachable;
    }
};

/// Trace span for storage operations
pub const TraceSpan = struct {
    context: TraceContext,
    operation_name: []const u8,
    start_time_ns: u64,
    end_time_ns: ?u64,
    attributes: std.StringHashMap([]const u8),
    allocator: std.mem.Allocator,

    const Self = @This();

    /// Create a new trace span
    pub fn init(allocator: std.mem.Allocator, context: TraceContext, operation_name: []const u8) Self {
        return Self{
            .context = context,
            .operation_name = operation_name,
            .start_time_ns = @intCast(std.time.nanoTimestamp()),
            .end_time_ns = null,
            .attributes = std.StringHashMap([]const u8).init(allocator),
            .allocator = allocator,
        };
    }

    /// Deinitialize the span
    pub fn deinit(self: *Self) void {
        self.attributes.deinit();
    }

    /// Add an attribute to the span
    pub fn setAttribute(self: *Self, key: []const u8, value: []const u8) !void {
        try self.attributes.put(key, value);
    }

    /// Finish the span
    pub fn finish(self: *Self) void {
        self.end_time_ns = @intCast(std.time.nanoTimestamp());
    }

    /// Get span duration in nanoseconds
    pub fn durationNs(self: *const Self) ?u64 {
        if (self.end_time_ns) |end| {
            return end - self.start_time_ns;
        }
        return null;
    }
};

/// Thread-local trace context
threadlocal var current_trace_context: ?TraceContext = null;

/// Set the current trace context for this thread
pub fn setCurrentContext(context: TraceContext) void {
    current_trace_context = context;
}

/// Get the current trace context for this thread
pub fn getCurrentContext() ?TraceContext {
    return current_trace_context;
}

/// Clear the current trace context
pub fn clearCurrentContext() void {
    current_trace_context = null;
}

/// Create a child span from current context
pub fn startSpan(allocator: std.mem.Allocator, operation_name: []const u8) ?TraceSpan {
    if (current_trace_context) |ctx| {
        const child_ctx = ctx.child();
        return TraceSpan.init(allocator, child_ctx, operation_name);
    }
    return null;
}

// C API for FFI with Rust
export fn owl_tracing_set_context(trace_id: [*]const u8, span_id: [*]const u8, parent_span_id: [*]const u8, flags: u8) void {
    var ctx = TraceContext{
        .trace_id = undefined,
        .span_id = undefined,
        .parent_span_id = undefined,
        .flags = flags,
    };

    @memcpy(&ctx.trace_id, trace_id[0..16]);
    @memcpy(&ctx.span_id, span_id[0..8]);
    @memcpy(&ctx.parent_span_id, parent_span_id[0..8]);

    setCurrentContext(ctx);
}

export fn owl_tracing_clear_context() void {
    clearCurrentContext();
}

export fn owl_tracing_get_context(trace_id_out: [*]u8, span_id_out: [*]u8, parent_span_id_out: [*]u8, flags_out: *u8) bool {
    if (getCurrentContext()) |ctx| {
        @memcpy(trace_id_out[0..16], &ctx.trace_id);
        @memcpy(span_id_out[0..8], &ctx.span_id);
        @memcpy(parent_span_id_out[0..8], &ctx.parent_span_id);
        flags_out.* = ctx.flags;
        return true;
    }
    return false;
}

test "trace context creation" {
    const ctx = TraceContext.root();
    try std.testing.expect(ctx.isSampled());

    const child_ctx = ctx.child();
    try std.testing.expectEqualSlices(u8, &ctx.trace_id, &child_ctx.trace_id);
    try std.testing.expectEqualSlices(u8, &ctx.span_id, &child_ctx.parent_span_id);
}

test "trace span" {
    const allocator = std.testing.allocator;
    const ctx = TraceContext.root();

    var span = TraceSpan.init(allocator, ctx, "test_operation");
    defer span.deinit();

    try span.setAttribute("key1", "value1");
    try span.setAttribute("key2", "value2");

    // Simulate some work
    var sum: u64 = 0;
    var i: u64 = 0;
    while (i < 1000000) : (i += 1) {
        sum +%= i;
    }

    span.finish();

    const duration = span.durationNs();
    try std.testing.expect(duration != null);
    try std.testing.expect(duration.? > 0);
}

test "thread local context" {
    const ctx = TraceContext.root();
    setCurrentContext(ctx);

    const retrieved = getCurrentContext();
    try std.testing.expect(retrieved != null);
    try std.testing.expectEqualSlices(u8, &ctx.trace_id, &retrieved.?.trace_id);

    clearCurrentContext();
    try std.testing.expect(getCurrentContext() == null);
}
