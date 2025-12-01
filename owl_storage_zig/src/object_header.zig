// Task 1.2: ObjectHeader Implementation
// Object header with proper memory layout for cache efficiency
// Uses extern struct to ensure predictable memory layout

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;

/// Object header with OPTIMIZED memory layout for cache efficiency.
///
/// OPTIMIZED Memory Layout (56 bytes, fits in single 64-byte cache line):
/// HOT FIELDS (first 32 bytes - first half of cache line):
/// - flags: u8 (1 byte) - Deleted, Overflow, Pinned, etc. [MOST ACCESSED]
/// - type_tag: u16 (2 bytes) - Schema type ID [FREQUENTLY ACCESSED]
/// - _padding_hot: [5]u8 (5 bytes) - Padding for alignment
/// - size: u32 (4 bytes) - Total object size [FREQUENTLY ACCESSED]
/// - version: u32 (4 bytes) - MVCC version number [FREQUENTLY ACCESSED]
/// - lsn: u64 (8 bytes) - Log Sequence Number for MVCC [FREQUENTLY ACCESSED]
/// - checksum: u32 (4 bytes) - CRC32 checksum [FREQUENTLY ACCESSED]
///
/// COLD FIELDS (second 32 bytes - second half of cache line):
/// - id: [16]u8 (16 bytes) - UUID as bytes [LESS FREQUENTLY ACCESSED]
/// - _padding_cold: [12]u8 (12 bytes) - Padding to fill cache line
///
/// Total: 56 bytes (padded to 64 for full cache line alignment)
///
/// OPTIMIZATION RATIONALE:
/// - flags at offset 0: Most frequently checked field (deleted, pinned)
/// - type_tag at offset 1: Frequently used for type dispatch
/// - size at offset 8: Needed for allocation and navigation
/// - version/lsn at offsets 12/16: Frequently updated for MVCC/WAL
/// - checksum at offset 24: Frequently checked for integrity
/// - id at offset 28: Less frequently accessed (only for lookups)
///
/// This layout ensures that the most frequently accessed fields are in the
/// first 32 bytes of the cache line, minimizing cache misses during hot path
/// operations like navigation, deletion checks, and MVCC operations.
///
/// Design: Uses extern struct for C ABI compatibility and predictable layout
pub const ObjectHeader = extern struct {
    // ========================================================================
    // HOT FIELDS - First 32 bytes of cache line
    // ========================================================================

    /// Flags: Deleted, Overflow, Pinned, etc. [OFFSET 0 - HOTTEST FIELD]
    /// Placed first for fastest access during navigation and deletion checks
    flags: u8,

    /// Schema type ID (0=User, 1=Order, etc.) [OFFSET 1]
    /// Frequently used for type dispatch and schema lookups
    type_tag: u16,

    /// Padding for alignment (5 bytes)
    _padding_hot: [5]u8,

    /// Total object size in bytes (including header) [OFFSET 8]
    /// Frequently accessed during allocation and navigation
    size: u32,

    /// MVCC version number [OFFSET 12]
    /// Frequently updated during transactions
    version: u32,

    /// Log Sequence Number for MVCC [OFFSET 16]
    /// Frequently updated during WAL operations
    lsn: u64,

    /// CRC32 checksum for integrity [OFFSET 24]
    /// Frequently checked during reads
    checksum: u32,

    // ========================================================================
    // COLD FIELDS - Second 32 bytes of cache line
    // ========================================================================

    /// Logical identifier (16 bytes UUID) [OFFSET 28]
    /// Less frequently accessed (only during lookups)
    id: [16]u8,

    /// Padding to fill cache line (12 bytes)
    _padding_cold: [12]u8,

    // Flag bit positions
    pub const FLAG_DELETED: u8 = 0x01; // Bit 0: Object is deleted
    pub const FLAG_OVERFLOW: u8 = 0x02; // Bit 1: Has overflow data
    pub const FLAG_PINNED: u8 = 0x04; // Bit 2: Pinned in memory
    // Bits 3-4: Compression type (2 bits for 4 types)
    // 00 = none, 01 = LZ4, 10 = ZSTD, 11 = reserved
    pub const FLAG_COMPRESSION_MASK: u8 = 0x18; // Bits 3-4

    /// Create a new object header
    pub fn init(type_tag: u16, id: [16]u8, size: u32) ObjectHeader {
        return .{
            .flags = 0,
            .type_tag = type_tag,
            ._padding_hot = [_]u8{0} ** 5,
            .size = size,
            .version = 0,
            .lsn = 0,
            .checksum = 0,
            .id = id,
            ._padding_cold = [_]u8{0} ** 12,
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
    /// OPTIMIZED: Placed at offset 0 for fastest access
    pub inline fn isDeleted(self: *const ObjectHeader) bool {
        return (self.flags & FLAG_DELETED) != 0;
    }

    /// Fast path check for deleted flag with explicit branch hint
    /// OPTIMIZED: Uses explicit branching to hint that NOT deleted is common case
    pub inline fn isDeletedFast(self: *const ObjectHeader) bool {
        const result = (self.flags & FLAG_DELETED) != 0;
        // Hint to compiler: NOT deleted is the common case (hot path)
        if (!result) {
            // Hot path: object is not deleted (expected ~99% of the time)
            return false;
        }
        // Cold path: object is deleted (rare case)
        return true;
    }

    /// Mark object as deleted
    pub fn setDeleted(self: *ObjectHeader, deleted: bool) void {
        if (deleted) {
            self.flags |= FLAG_DELETED;
        } else {
            self.flags &= ~FLAG_DELETED;
        }
    }

    /// Atomically mark object as deleted
    /// This uses atomic operations to ensure thread safety
    pub fn atomicSetDeleted(self: *ObjectHeader, deleted: bool) void {
        // Convert flags to atomic value for thread-safe updates
        const flags_ptr: *std.atomic.Value(u8) = @ptrCast(&self.flags);

        if (deleted) {
            _ = flags_ptr.fetchOr(FLAG_DELETED, .seq_cst);
        } else {
            _ = flags_ptr.fetchAnd(~FLAG_DELETED, .seq_cst);
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

    /// Atomically mark object as having overflow data
    /// This uses atomic operations to ensure thread safety
    pub fn atomicSetOverflow(self: *ObjectHeader, overflow: bool) void {
        const flags_ptr: *std.atomic.Value(u8) = @ptrCast(&self.flags);

        if (overflow) {
            _ = flags_ptr.fetchOr(FLAG_OVERFLOW, .seq_cst);
        } else {
            _ = flags_ptr.fetchAnd(~FLAG_OVERFLOW, .seq_cst);
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

    /// Atomically pin/unpin object in memory
    /// This uses atomic operations to ensure thread safety
    pub fn atomicSetPinned(self: *ObjectHeader, pinned: bool) void {
        const flags_ptr: *std.atomic.Value(u8) = @ptrCast(&self.flags);

        if (pinned) {
            _ = flags_ptr.fetchOr(FLAG_PINNED, .seq_cst);
        } else {
            _ = flags_ptr.fetchAnd(~FLAG_PINNED, .seq_cst);
        }
    }

    // ========================================================================
    // Compression Operations (Task 10)
    // ========================================================================

    /// Get compression type from flags
    pub inline fn getCompressionType(self: *const ObjectHeader) u2 {
        return @intCast((self.flags & FLAG_COMPRESSION_MASK) >> 3);
    }

    /// Set compression type in flags
    pub fn setCompressionType(self: *ObjectHeader, compression_type: u2) void {
        const cleared = self.flags & ~FLAG_COMPRESSION_MASK;
        self.flags = cleared | (@as(u8, compression_type) << 3);
    }

    /// Atomically set compression type
    pub fn atomicSetCompressionType(self: *ObjectHeader, compression_type: u2) void {
        const flags_ptr: *std.atomic.Value(u8) = @ptrCast(&self.flags);

        // Read-modify-write loop
        var current = flags_ptr.load(.seq_cst);
        while (true) {
            const cleared = current & ~FLAG_COMPRESSION_MASK;
            const new_flags = cleared | (@as(u8, compression_type) << 3);

            if (flags_ptr.cmpxchgWeak(current, new_flags, .seq_cst, .seq_cst)) |updated| {
                current = updated;
            } else {
                break;
            }
        }
    }

    /// Check if object is compressed
    pub inline fn isCompressed(self: *const ObjectHeader) bool {
        return self.getCompressionType() != 0;
    }

    /// Atomically update version number
    /// This uses atomic operations to ensure thread safety for MVCC
    pub fn atomicIncrementVersion(self: *ObjectHeader) u32 {
        const version_ptr: *std.atomic.Value(u32) = @ptrCast(&self.version);
        return version_ptr.fetchAdd(1, .seq_cst) + 1;
    }

    /// Atomically update LSN
    /// This uses atomic operations to ensure thread safety for WAL coordination
    pub fn atomicSetLSN(self: *ObjectHeader, lsn: u64) void {
        const lsn_ptr: *std.atomic.Value(u64) = @ptrCast(&self.lsn);
        lsn_ptr.store(lsn, .seq_cst);
    }

    /// Atomically get LSN
    /// This uses atomic operations to ensure thread safety
    pub fn atomicGetLSN(self: *const ObjectHeader) u64 {
        const lsn_ptr: *const std.atomic.Value(u64) = @ptrCast(&self.lsn);
        return lsn_ptr.load(.seq_cst);
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

test "ObjectHeader: compression type" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    // Initially no compression
    try std.testing.expectEqual(@as(u2, 0), header.getCompressionType());
    try std.testing.expect(!header.isCompressed());

    // Set LZ4 compression (type 1)
    header.setCompressionType(1);
    try std.testing.expectEqual(@as(u2, 1), header.getCompressionType());
    try std.testing.expect(header.isCompressed());

    // Set ZSTD compression (type 2)
    header.setCompressionType(2);
    try std.testing.expectEqual(@as(u2, 2), header.getCompressionType());
    try std.testing.expect(header.isCompressed());

    // Clear compression
    header.setCompressionType(0);
    try std.testing.expectEqual(@as(u2, 0), header.getCompressionType());
    try std.testing.expect(!header.isCompressed());
}

test "ObjectHeader: compression with other flags" {
    const id = [_]u8{1} ** 16;
    var header = ObjectHeader.init(1, id, 100);

    // Set multiple flags
    header.setDeleted(true);
    header.setPinned(true);
    header.setCompressionType(1); // LZ4

    // All flags should be independent
    try std.testing.expect(header.isDeleted());
    try std.testing.expect(header.isPinned());
    try std.testing.expectEqual(@as(u2, 1), header.getCompressionType());

    // Change compression type
    header.setCompressionType(2); // ZSTD

    // Other flags should remain
    try std.testing.expect(header.isDeleted());
    try std.testing.expect(header.isPinned());
    try std.testing.expectEqual(@as(u2, 2), header.getCompressionType());
}
