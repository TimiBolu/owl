// Logging Support for Zig Storage Layer
// Provides structured logging with correlation ID propagation

const std = @import("std");

/// Log level
pub const LogLevel = enum(u8) {
    trace = 0,
    debug = 1,
    info = 2,
    warn = 3,
    err = 4,

    pub fn toString(self: LogLevel) []const u8 {
        return switch (self) {
            .trace => "TRACE",
            .debug => "DEBUG",
            .info => "INFO",
            .warn => "WARN",
            .err => "ERROR",
        };
    }
};

/// Correlation ID for cross-layer tracking
pub const CorrelationId = struct {
    bytes: [36]u8, // UUID string format

    const Self = @This();

    /// Create a new correlation ID
    pub fn init() Self {
        var id = Self{ .bytes = undefined };

        // Generate a simple UUID-like string using timestamp
        const timestamp = @as(u64, @intCast(std.time.nanoTimestamp()));
        const formatted = std.fmt.bufPrint(&id.bytes, "{x:0>8}-{x:0>4}-{x:0>4}-{x:0>4}-{x:0>12}", .{
            @as(u32, @truncate(timestamp >> 32)),
            @as(u16, @truncate(timestamp >> 16)),
            @as(u16, @truncate(timestamp)),
            @as(u16, @truncate(timestamp >> 48)),
            timestamp & 0xFFFFFFFFFFFF,
        }) catch unreachable;

        std.debug.assert(formatted.len == 36);
        return id;
    }

    /// Create from bytes
    pub fn fromBytes(bytes: [36]u8) Self {
        return Self{ .bytes = bytes };
    }

    /// Get as slice
    pub fn asSlice(self: *const Self) []const u8 {
        return &self.bytes;
    }
};

/// Log entry for FFI with Rust
pub const LogEntry = extern struct {
    correlation_id: [36]u8,
    level: u8,
    message: [*]const u8,
    message_len: usize,
    component: [*]const u8,
    component_len: usize,
    duration_ms: u64,
};

/// Logger for Zig components
pub const Logger = struct {
    component: []const u8,
    correlation_id: ?CorrelationId,

    const Self = @This();

    /// Create a new logger
    pub fn init(component: []const u8) Self {
        return Self{
            .component = component,
            .correlation_id = null,
        };
    }

    /// Set correlation ID
    pub fn setCorrelationId(self: *Self, id: CorrelationId) void {
        self.correlation_id = id;
    }

    /// Get or create correlation ID
    fn getCorrelationId(self: *const Self) CorrelationId {
        return self.correlation_id orelse CorrelationId.init();
    }

    /// Log a message
    pub fn log(self: *const Self, level: LogLevel, message: []const u8) void {
        const cid = self.getCorrelationId();

        const entry = LogEntry{
            .correlation_id = cid.bytes,
            .level = @intFromEnum(level),
            .message = message.ptr,
            .message_len = message.len,
            .component = self.component.ptr,
            .component_len = self.component.len,
            .duration_ms = 0,
        };

        owl_log_from_zig(&entry);
    }

    /// Log with duration
    pub fn logWithDuration(self: *const Self, level: LogLevel, message: []const u8, duration_ms: u64) void {
        const cid = self.getCorrelationId();

        const entry = LogEntry{
            .correlation_id = cid.bytes,
            .level = @intFromEnum(level),
            .message = message.ptr,
            .message_len = message.len,
            .component = self.component.ptr,
            .component_len = self.component.len,
            .duration_ms = duration_ms,
        };

        owl_log_from_zig(&entry);
    }

    /// Convenience methods
    pub fn trace(self: *const Self, message: []const u8) void {
        self.log(.trace, message);
    }

    pub fn debug(self: *const Self, message: []const u8) void {
        self.log(.debug, message);
    }

    pub fn info(self: *const Self, message: []const u8) void {
        self.log(.info, message);
    }

    pub fn warn(self: *const Self, message: []const u8) void {
        self.log(.warn, message);
    }

    pub fn err(self: *const Self, message: []const u8) void {
        self.log(.err, message);
    }

    /// Create a timed operation
    pub fn timedOperation(self: *const Self, operation: []const u8) TimedOperation {
        return TimedOperation{
            .logger = self,
            .operation = operation,
            .start_ns = @intCast(std.time.nanoTimestamp()),
        };
    }
};

/// Timed operation that logs duration
pub const TimedOperation = struct {
    logger: *const Logger,
    operation: []const u8,
    start_ns: u64,

    const Self = @This();

    /// Finish the operation
    pub fn finish(self: *const Self) void {
        const end_ns: u64 = @intCast(std.time.nanoTimestamp());
        const duration_ms = (end_ns - self.start_ns) / 1_000_000;

        var buf: [256]u8 = undefined;
        const message = std.fmt.bufPrint(&buf, "Operation '{s}' completed", .{self.operation}) catch "Operation completed";

        self.logger.logWithDuration(.info, message, duration_ms);
    }

    /// Finish with error
    pub fn finishWithError(self: *const Self, error_msg: []const u8) void {
        const end_ns: u64 = @intCast(std.time.nanoTimestamp());
        const duration_ms = (end_ns - self.start_ns) / 1_000_000;

        var buf: [512]u8 = undefined;
        const message = std.fmt.bufPrint(&buf, "Operation '{s}' failed: {s}", .{ self.operation, error_msg }) catch "Operation failed";

        self.logger.logWithDuration(.err, message, duration_ms);
    }
};

// Thread-local logger
threadlocal var current_logger: ?*const Logger = null;

/// Set the current thread-local logger
pub fn setCurrentLogger(logger: *const Logger) void {
    current_logger = logger;
}

/// Get the current thread-local logger
pub fn getCurrentLogger() ?*const Logger {
    return current_logger;
}

/// Clear the current logger
pub fn clearCurrentLogger() void {
    current_logger = null;
}

// FFI declaration for Rust logging function
extern "C" fn owl_log_from_zig(entry: *const LogEntry) void;

test "correlation id" {
    const id1 = CorrelationId.init();
    const id2 = CorrelationId.init();

    try std.testing.expect(id1.asSlice().len == 36);
    try std.testing.expect(id2.asSlice().len == 36);
}

test "logger" {
    var logger = Logger.init("test_component");
    const cid = CorrelationId.init();
    logger.setCorrelationId(cid);

    // Note: These will try to call FFI, which won't work in tests
    // In production, these would log to Rust layer
    // logger.info("Test message");
    // logger.debug("Debug message");
}

test "timed operation" {
    const logger = Logger.init("test");
    const op = logger.timedOperation("test_op");

    // Simulate some work
    var sum: u64 = 0;
    var i: u64 = 0;
    while (i < 1000) : (i += 1) {
        sum +%= i;
    }

    // Note: finish() would call FFI in production
    // op.finish();
    _ = op;
}
