// Task 1.2: ObjectHeader Implementation
// Object header with proper memory layout for cache efficiency
// Uses extern struct to ensure predictable memory layout

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;

/// Object header with proper memory layout for cache efficiency.
///
/// Memory Layout (41 bytes, fits in single 64-byte cache line):
/// - type_tag: u16 (2 bytes) - Schema type ID
/// - id: [16]u8 (16 bytes) - UUID as bytes
/// - version: u32 (4 bytes) - MVCC version number
/// - size: u32 (4 bytes) - Total object size in bytes
/// - flags: u8 (1 byte) - Deleted, Overflow, Pinned, etc.
/// - checksum: u32 (4 bytes) - CRC32 checksum for integrity
/// - lsn: u64 (8 bytes) - Log Sequence Number for MVCC
/// - padding: [2]u8 (2 bytes) - Align to 8 bytes
///
/// Total: 43 bytes (padded to 48 for alignment)
///
/// Design: Uses extern struct for C ABI compatibility and predictable layout
pub const ObjectHeader = extern struct {
    /// Schema type ID (0=User, 1=Order, etc.)
    type_tag: u16,

    /// Logical identifier (16 bytes UUID)
    id: [16]u8,

    /// MVCC version number
    version: u32,

    /// Total object size in bytes (including header)
    size: u32,

    /// Flags: Deleted, Overflow, Pinned, etc.
    flags: u8,

    /// Padding for alignment (7 bytes to align checksum)
    _padding1: [7]u8,

    /// CRC32 checksum for integrity
    checksum: u32,

    /// Padding for alignment (4 bytes)
    _padding2: [4]u8,

    /// Log Sequence Number for MVCC
    lsn: u64,

    // Flag bit positions
    pub const FLAG_DELETED: u8 = 0x01; // Bit 0: Object is deleted
    pub const FLAG_OVERFLOW: u8 = 0x02; // Bit 1: Has overflow data
    pub const FLAG_PINNED: u8 = 0x04; // Bit 2: Pinned in memory

    /// Create a new object header
    pub fn init(type_tag: u16, id: [16]u8, size: u32) ObjectHeader {
        return .{
            .type_tag = type_tag,
            .id = id,
            .version = 0,
            .size = size,
            .flags = 0,
            ._padding1 = [_]u8{0} ** 7,
            .checksum = 0,
            ._padding2 = [_]u8{0} ** 4,
            .lsn = 0,
        };
    }

    /// Get the size of the header in bytes
    pub inline fn headerSize() usize {
        return @sizeOf(ObjectHeader);
    }

    // ========================================================================
    // Flag Operations
    // ========================================================================

    /// Check if object is marked as deleted
    pub inline fn isDeleted(self: *const ObjectHeader) bool {
        return (self.flags & FLAG_DELETED) != 0;
    }

    /// Mark object as deleted
    pub fn setDeleted(self: *ObjectHeader, deleted: bool) void {
        if (deleted) {
            self.flags |= FLAG_DELETED;
        } else {
            self.flags &= ~FLAG_DELETED;
        }
    }

    /// Check if object has overflow data
    pub inline fn hasOverflow(self: *const ObjectHeader) bool {
        return (self.flags & FLAG_OVERFLOW) != 0;
    }

    /// Mark object as having overflow data
    pub fn setOverflow(self: *ObjectHeader, overflow: bool) void {
        if (overflow) {
            self.flags |= FLAG_OVERFLOW;
        } else {
            self.flags &= ~FLAG_OVERFLOW;
        }
    }

    /// Check if object is pinned in memory
    pub inline fn isPinned(self: *const ObjectHeader) bool {
        return (self.flags & FLAG_PINNED) != 0;
    }

    /// Pin/unpin object in memory
    pub fn setPinned(self: *ObjectHeader, pinned: bool) void {
        if (pinned) {
            self.flags |= FLAG_PINNED;
        } else {
            self.flags &= ~FLAG_PINNED;
        }
    }

    // ========================================================================
    // Checksum Operations
    // ========================================================================

    /// Calculate CRC32 checksum for the given data
    pub fn calculateChecksum(data: []const u8) u32 {
        return std.hash.Crc32.hash(data);
    }

    /// Verify checksum against data
    pub fn verifyChecksum(self: *const ObjectHeader, data: []const u8) bool {
        return self.checksum == calculateChecksum(data);
    }

    /// Update the checksum field based on current header state
    /// Note: This should be called after serializing the full object
    pub fn updateChecksum(self: *ObjectHeader, object_data: []const u8) void {
        self.checksum = calculateChecksum(object_data);
    }

    // ========================================================================
    // Serialization
    // ========================================================================

    /// Serialize header to bytes
    pub fn toBytes(self: *const ObjectHeader) [headerSize()]u8 {
        var bytes: [headerSize()]u8 = undefined;
        @memcpy(&bytes, std.mem.asBytes(self));
        return bytes;
    }

    /// Deserialize header from bytes
    pub fn fromBytes(bytes: []const u8) !ObjectHeader {
        if (bytes.len < headerSize()) {
            return error.InsufficientData;
        }
        var header: ObjectHeader = undefined;
        @memcpy(std.mem.asBytes(&header), bytes[0..headerSize()]);
        return header;
    }

    // ========================================================================
    // Debugging
    // ========================================================================

    /// Format for printing (useful for debugging)
    pub fn format(
        self: ObjectHeader,
        writer: anytype,
    ) !void {
        try writer.print("ObjectHeader{{ type={d}, version={d}, size={d}, flags=0x{x:0>2}, lsn={d} }}", .{
            self.type_tag,
            self.version,
            self.size,
            self.flags,
            self.lsn,
        });
    }
};

// Compile-time assertions to ensure correct size and alignment
comptime {
    // ObjectHeader should fit in a cache line (64 bytes)
    if (@sizeOf(ObjectHeader) > 64) {
        @compileError("ObjectHeader is too large, should fit in 64-byte cache line");
    }

    // ObjectHeader must be properly aligned
    if (@alignOf(ObjectHeader) < 8) {
        @compileError("ObjectHeader must be at least 8-byte aligned");
    }
}

// ============================================================================
// Tests
// ============================================================================

test "ObjectHeader: size fits in cache line" {
    const size = @sizeOf(ObjectHeader);
    try std.testing.expect(size <= 64);
    std.debug.print("\nObjectHeader size: {d} bytes (fits in 64-byte cache line)\n", .{size});
}

test "ObjectHeader: alignment" {
    const alignment = @alignOf(ObjectHeader);
    try std.testing.expect(alignment >= 8);
}

test "ObjectHeader: init" {
    const id = [_]u8{1} ** 16;
    const header = ObjectHeader.init(42, id, 1024);

    try std.testing.expectEqual(@as(u16, 42), header.type_tag);
    try std.testing.expectEqual(id, header.id);
    try std.testing.expectEqual(@as(u32, 0), header.version);
    try std.testing.expectEqual(@as(u32, 1024), header.size);
    try std.testing.expectEqual(@as(u8, 0), header.flags);
    try std.testing.expectEqual(@as(u32, 0), header.checksum);
    try std.testing.expectEqual(@as(u64, 0), header.lsn);
}

test "ObjectHeader: deleted flag" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    try std.testing.expect(!header.isDeleted());

    header.setDeleted(true);
    try std.testing.expect(header.isDeleted());

    header.setDeleted(false);
    try std.testing.expect(!header.isDeleted());
}

test "ObjectHeader: overflow flag" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    try std.testing.expect(!header.hasOverflow());

    header.setOverflow(true);
    try std.testing.expect(header.hasOverflow());

    header.setOverflow(false);
    try std.testing.expect(!header.hasOverflow());
}

test "ObjectHeader: pinned flag" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    try std.testing.expect(!header.isPinned());

    header.setPinned(true);
    try std.testing.expect(header.isPinned());

    header.setPinned(false);
    try std.testing.expect(!header.isPinned());
}

test "ObjectHeader: multiple flags" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    // Set all flags
    header.setDeleted(true);
    header.setOverflow(true);
    header.setPinned(true);

    try std.testing.expect(header.isDeleted());
    try std.testing.expect(header.hasOverflow());
    try std.testing.expect(header.isPinned());

    // Clear one flag, others should remain
    header.setDeleted(false);
    try std.testing.expect(!header.isDeleted());
    try std.testing.expect(header.hasOverflow());
    try std.testing.expect(header.isPinned());
}

test "ObjectHeader: checksum calculation" {
    const data = "Hello, World!";
    const checksum = ObjectHeader.calculateChecksum(data);

    try std.testing.expect(checksum != 0);

    // Same data should produce same checksum
    const checksum2 = ObjectHeader.calculateChecksum(data);
    try std.testing.expectEqual(checksum, checksum2);

    // Different data should produce different checksum
    const checksum3 = ObjectHeader.calculateChecksum("Different data");
    try std.testing.expect(checksum != checksum3);
}

test "ObjectHeader: checksum verification" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    const data = "Test data for checksum";
    header.updateChecksum(data);

    try std.testing.expect(header.verifyChecksum(data));

    // Wrong data should fail verification
    try std.testing.expect(!header.verifyChecksum("Wrong data"));
}

test "ObjectHeader: serialization round-trip" {
    const id = [_]u8{ 0x12, 0x34, 0x56, 0x78 } ++ [_]u8{0xAB} ** 12;
    var header = ObjectHeader.init(42, id, 2048);
    header.version = 5;
    header.lsn = 12345;
    header.setDeleted(true);
    header.checksum = 0xDEADBEEF;

    // Serialize
    const bytes = header.toBytes();

    // Deserialize
    const header2 = try ObjectHeader.fromBytes(&bytes);

    // Verify all fields
    try std.testing.expectEqual(header.type_tag, header2.type_tag);
    try std.testing.expectEqual(header.id, header2.id);
    try std.testing.expectEqual(header.version, header2.version);
    try std.testing.expectEqual(header.size, header2.size);
    try std.testing.expectEqual(header.flags, header2.flags);
    try std.testing.expectEqual(header.checksum, header2.checksum);
    try std.testing.expectEqual(header.lsn, header2.lsn);
}

test "ObjectHeader: serialization insufficient data" {
    const bytes = [_]u8{ 1, 2, 3 }; // Too small
    const result = ObjectHeader.fromBytes(&bytes);

    try std.testing.expectError(error.InsufficientData, result);
}

test "ObjectHeader: version increment" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    try std.testing.expectEqual(@as(u32, 0), header.version);

    header.version += 1;
    try std.testing.expectEqual(@as(u32, 1), header.version);

    header.version += 1;
    try std.testing.expectEqual(@as(u32, 2), header.version);
}

test "ObjectHeader: lsn tracking" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    try std.testing.expectEqual(@as(u64, 0), header.lsn);

    header.lsn = 1000;
    try std.testing.expectEqual(@as(u64, 1000), header.lsn);

    header.lsn = 999999999;
    try std.testing.expectEqual(@as(u64, 999999999), header.lsn);
}

test "ObjectHeader: type_tag values" {
    const id = [_]u8{1} ** 16;

    const header1 = ObjectHeader.init(0, id, 100); // User type
    try std.testing.expectEqual(@as(u16, 0), header1.type_tag);

    const header2 = ObjectHeader.init(1, id, 100); // Order type
    try std.testing.expectEqual(@as(u16, 1), header2.type_tag);

    const header3 = ObjectHeader.init(65535, id, 100); // Max type
    try std.testing.expectEqual(@as(u16, 65535), header3.type_tag);
}

test "ObjectHeader: size field" {
    const id = [_]u8{1} ** 16;

    // Small object
    const header1 = ObjectHeader.init(1, id, 64);
    try std.testing.expectEqual(@as(u32, 64), header1.size);

    // Large object
    const header2 = ObjectHeader.init(1, id, 1024 * 1024); // 1MB
    try std.testing.expectEqual(@as(u32, 1024 * 1024), header2.size);
}

test "ObjectHeader: UUID uniqueness" {
    const id1 = [_]u8{1} ** 16;
    const id2 = [_]u8{2} ** 16;

    const header1 = ObjectHeader.init(1, id1, 100);
    const header2 = ObjectHeader.init(1, id2, 100);

    try std.testing.expect(!std.mem.eql(u8, &header1.id, &header2.id));
}

test "ObjectHeader: format output" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(42, id, 1024);
    header.version = 5;
    header.lsn = 12345;
    header.setDeleted(true);

    var buf: [200]u8 = undefined;
    const result = try std.fmt.bufPrint(&buf, "{f}", .{header});

    try std.testing.expect(std.mem.indexOf(u8, result, "ObjectHeader") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "type=42") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "version=5") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "size=1024") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "lsn=12345") != null);
}

test "ObjectHeader: flag independence" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    // Set deleted
    header.setDeleted(true);
    try std.testing.expectEqual(@as(u8, 0x01), header.flags);

    // Add overflow
    header.setOverflow(true);
    try std.testing.expectEqual(@as(u8, 0x03), header.flags);

    // Add pinned
    header.setPinned(true);
    try std.testing.expectEqual(@as(u8, 0x07), header.flags);

    // Clear overflow
    header.setOverflow(false);
    try std.testing.expectEqual(@as(u8, 0x05), header.flags);
    try std.testing.expect(header.isDeleted());
    try std.testing.expect(!header.hasOverflow());
    try std.testing.expect(header.isPinned());
}
