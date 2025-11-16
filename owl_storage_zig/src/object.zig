// Task 1.3: Object Structure
// Complete object structure with header, fixed fields, variable data, and references

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;
const ObjectHeader = @import("object_header.zig").ObjectHeader;

/// Complete object structure with header, fixed fields, variable data, and references.
///
/// Memory Layout:
/// - header: ObjectHeader (48 bytes)
/// - fixed_len: u32 (4 bytes)
/// - var_len: u32 (4 bytes)
/// - refs_count: u32 (4 bytes)
/// - fixed: []u8 (variable)
/// - var_data: []u8 (variable)
/// - refs: []ObjRef (variable)
///
/// Design: Uses explicit allocator, no hidden allocations
pub const Object = struct {
    /// Object header with metadata
    header: ObjectHeader,
    /// Fixed-size fields (aligned for cache efficiency)
    fixed: []u8,
    /// CBOR-encoded nested/variable data
    var_data: []u8,
    /// Array of references to other objects
    refs: []ObjRef,
    /// Allocator for memory management
    allocator: std.mem.Allocator,

    /// Create a new object
    pub fn init(
        allocator: std.mem.Allocator,
        type_tag: u16,
        id: [16]u8,
        fixed: []const u8,
        var_data: []const u8,
        refs: []const ObjRef,
    ) !Object {
        // Calculate total size
        const size = ObjectHeader.headerSize() +
            12 + // 3 x 4-byte length fields
            fixed.len +
            var_data.len +
            (refs.len * @sizeOf(ObjRef));

        // Allocate and copy data
        const fixed_copy = try allocator.dupe(u8, fixed);
        errdefer allocator.free(fixed_copy);

        const var_copy = try allocator.dupe(u8, var_data);
        errdefer allocator.free(var_copy);

        const refs_copy = try allocator.dupe(ObjRef, refs);
        errdefer allocator.free(refs_copy);

        return .{
            .header = ObjectHeader.init(type_tag, id, @intCast(size)),
            .fixed = fixed_copy,
            .var_data = var_copy,
            .refs = refs_copy,
            .allocator = allocator,
        };
    }

    /// Free all allocated memory
    pub fn deinit(self: *Object) void {
        if (self.fixed.len > 0) self.allocator.free(self.fixed);
        if (self.var_data.len > 0) self.allocator.free(self.var_data);
        if (self.refs.len > 0) self.allocator.free(self.refs);
    }

    /// Get total size of the object in bytes
    pub fn totalSize(self: *const Object) usize {
        return self.header.size;
    }

    /// Get reference by index (for navigation)
    pub fn getReference(self: *const Object, index: usize) ?ObjRef {
        if (index >= self.refs.len) return null;
        return self.refs[index];
    }

    /// Serialize object to bytes for storage
    pub fn toBytes(self: *const Object, allocator: std.mem.Allocator) ![]u8 {
        var bytes = try std.ArrayList(u8).initCapacity(allocator, self.totalSize());
        errdefer bytes.deinit(allocator);

        // Serialize header
        const header_bytes = self.header.toBytes();
        try bytes.appendSlice(allocator, &header_bytes);

        // Write lengths
        const writer = bytes.writer(allocator);
        try writer.writeInt(u32, @intCast(self.fixed.len), .little);
        try writer.writeInt(u32, @intCast(self.var_data.len), .little);
        try writer.writeInt(u32, @intCast(self.refs.len), .little);

        // Serialize data
        try bytes.appendSlice(allocator, self.fixed);
        try bytes.appendSlice(allocator, self.var_data);

        // Serialize references
        for (self.refs) |ref| {
            try writer.writeInt(u64, ref.getOffset(), .little);
        }

        return bytes.toOwnedSlice(allocator);
    }

    /// Deserialize object from bytes
    pub fn fromBytes(allocator: std.mem.Allocator, bytes: []const u8) !Object {
        if (bytes.len < ObjectHeader.headerSize()) {
            return error.InsufficientData;
        }

        // Deserialize header
        const header = try ObjectHeader.fromBytes(bytes);
        var offset: usize = ObjectHeader.headerSize();

        // Read lengths
        if (bytes.len < offset + 12) {
            return error.InsufficientData;
        }

        const fixed_len = std.mem.readInt(u32, bytes[offset..][0..4], .little);
        offset += 4;
        const var_len = std.mem.readInt(u32, bytes[offset..][0..4], .little);
        offset += 4;
        const refs_count = std.mem.readInt(u32, bytes[offset..][0..4], .little);
        offset += 4;

        // Deserialize fixed fields
        if (bytes.len < offset + fixed_len) {
            return error.InsufficientData;
        }
        const fixed = try allocator.dupe(u8, bytes[offset..][0..fixed_len]);
        errdefer allocator.free(fixed);
        offset += fixed_len;

        // Deserialize variable data
        if (bytes.len < offset + var_len) {
            return error.InsufficientData;
        }
        const var_data = try allocator.dupe(u8, bytes[offset..][0..var_len]);
        errdefer allocator.free(var_data);
        offset += var_len;

        // Deserialize references
        const refs = try allocator.alloc(ObjRef, refs_count);
        errdefer allocator.free(refs);

        for (refs) |*ref| {
            if (bytes.len < offset + 8) {
                return error.InsufficientData;
            }
            const ref_offset = std.mem.readInt(u64, bytes[offset..][0..8], .little);
            ref.* = ObjRef.init(ref_offset);
            offset += 8;
        }

        return .{
            .header = header,
            .fixed = fixed,
            .var_data = var_data,
            .refs = refs,
            .allocator = allocator,
        };
    }

    /// Update checksum based on current data
    pub fn updateChecksum(self: *Object) !void {
        // Temporarily set checksum to 0 for calculation
        self.header.checksum = 0;

        const bytes = try self.toBytes(self.allocator);
        defer self.allocator.free(bytes);

        self.header.updateChecksum(bytes);
    }

    /// Verify object integrity
    pub fn verify(self: Object) !void {
        const stored_checksum = self.header.checksum;

        // Create a copy with checksum set to 0 for verification
        var temp = self;
        temp.header.checksum = 0;

        const bytes = try temp.toBytes(self.allocator);
        defer self.allocator.free(bytes);

        const calculated = ObjectHeader.calculateChecksum(bytes);
        if (stored_checksum != calculated) {
            return error.ChecksumMismatch;
        }
    }
};

// ============================================================================
// Tests
// ============================================================================

test "Object: creation and cleanup" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{ 1, 2, 3, 4 };
    const var_data = "test data";
    const refs = [_]ObjRef{ ObjRef.init(100), ObjRef.init(200) };

    var obj = try Object.init(allocator, 1, id, &fixed, var_data, &refs);
    defer obj.deinit();

    try std.testing.expectEqual(@as(u16, 1), obj.header.type_tag);
    try std.testing.expectEqual(id, obj.header.id);
    try std.testing.expectEqual(@as(usize, 4), obj.fixed.len);
    try std.testing.expectEqual(@as(usize, 9), obj.var_data.len);
    try std.testing.expectEqual(@as(usize, 2), obj.refs.len);
}

test "Object: getReference" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const refs = [_]ObjRef{ ObjRef.init(100), ObjRef.init(200), ObjRef.init(300) };

    var obj = try Object.init(allocator, 1, id, &[_]u8{}, &[_]u8{}, &refs);
    defer obj.deinit();

    const ref0 = obj.getReference(0);
    try std.testing.expect(ref0 != null);
    try std.testing.expectEqual(@as(u64, 100), ref0.?.getOffset());

    const ref1 = obj.getReference(1);
    try std.testing.expect(ref1 != null);
    try std.testing.expectEqual(@as(u64, 200), ref1.?.getOffset());

    const ref_invalid = obj.getReference(10);
    try std.testing.expect(ref_invalid == null);
}

test "Object: serialization round-trip" {
    const allocator = std.testing.allocator;

    const id = [_]u8{ 0x12, 0x34, 0x56, 0x78 } ++ [_]u8{0xAB} ** 12;
    const fixed = [_]u8{ 1, 2, 3, 4, 5 };
    const var_data = "Hello, World!";
    const refs = [_]ObjRef{ ObjRef.init(1000), ObjRef.init(2000) };

    var obj = try Object.init(allocator, 42, id, &fixed, var_data, &refs);
    defer obj.deinit();

    // Serialize
    const bytes = try obj.toBytes(allocator);
    defer allocator.free(bytes);

    try std.testing.expect(bytes.len >= ObjectHeader.headerSize());

    // Deserialize
    var obj2 = try Object.fromBytes(allocator, bytes);
    defer obj2.deinit();

    // Verify
    try std.testing.expectEqual(obj.header.type_tag, obj2.header.type_tag);
    try std.testing.expectEqual(obj.header.id, obj2.header.id);
    try std.testing.expectEqualSlices(u8, obj.fixed, obj2.fixed);
    try std.testing.expectEqualSlices(u8, obj.var_data, obj2.var_data);
    try std.testing.expectEqual(obj.refs.len, obj2.refs.len);
    for (obj.refs, obj2.refs) |r1, r2| {
        try std.testing.expectEqual(r1.getOffset(), r2.getOffset());
    }
}

test "Object: empty data" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{}, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    try std.testing.expectEqual(@as(usize, 0), obj.fixed.len);
    try std.testing.expectEqual(@as(usize, 0), obj.var_data.len);
    try std.testing.expectEqual(@as(usize, 0), obj.refs.len);
}

test "Object: large data" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{0xFF} ** 1024;
    const var_data = [_]u8{0xAA} ** 2048;
    var refs: [50]ObjRef = undefined;
    for (&refs, 0..) |*ref, i| {
        ref.* = ObjRef.init(i * 100);
    }

    var obj = try Object.init(allocator, 1, id, &fixed, &var_data, &refs);
    defer obj.deinit();

    try std.testing.expectEqual(@as(usize, 1024), obj.fixed.len);
    try std.testing.expectEqual(@as(usize, 2048), obj.var_data.len);
    try std.testing.expectEqual(@as(usize, 50), obj.refs.len);
}

test "Object: totalSize" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{ 1, 2, 3, 4 };
    const var_data = "test";
    const refs = [_]ObjRef{ObjRef.init(100)};

    var obj = try Object.init(allocator, 1, id, &fixed, var_data, &refs);
    defer obj.deinit();

    const expected_size = ObjectHeader.headerSize() + 12 + 4 + 4 + 8;
    try std.testing.expectEqual(expected_size, obj.totalSize());
}

test "Object: checksum update and verify" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{ 1, 2, 3, 4 };
    var obj = try Object.init(allocator, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    // Update checksum
    try obj.updateChecksum();
    try std.testing.expect(obj.header.checksum != 0);

    // Verify should pass
    try obj.verify();
}

test "Object: checksum mismatch detection" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 1, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    try obj.updateChecksum();

    // Corrupt the data
    obj.fixed[0] = 99;

    // Verify should fail
    const result = obj.verify();
    try std.testing.expectError(error.ChecksumMismatch, result);
}

test "Object: serialization insufficient data" {
    const allocator = std.testing.allocator;

    const bytes = [_]u8{ 1, 2, 3 }; // Too small
    const result = Object.fromBytes(allocator, &bytes);

    try std.testing.expectError(error.InsufficientData, result);
}

test "Object: multiple references" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const refs = [_]ObjRef{
        ObjRef.init(100),
        ObjRef.init(200),
        ObjRef.init(300),
        ObjRef.init(400),
        ObjRef.init(500),
    };

    var obj = try Object.init(allocator, 1, id, &[_]u8{}, &[_]u8{}, &refs);
    defer obj.deinit();

    for (refs, 0..) |expected_ref, i| {
        const actual_ref = obj.getReference(i);
        try std.testing.expect(actual_ref != null);
        try std.testing.expectEqual(expected_ref.getOffset(), actual_ref.?.getOffset());
    }
}

test "Object: null references" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const refs = [_]ObjRef{ ObjRef.nullRef(), ObjRef.init(100), ObjRef.nullRef() };

    var obj = try Object.init(allocator, 1, id, &[_]u8{}, &[_]u8{}, &refs);
    defer obj.deinit();

    const ref0 = obj.getReference(0).?;
    try std.testing.expect(ref0.isNull());

    const ref1 = obj.getReference(1).?;
    try std.testing.expect(!ref1.isNull());

    const ref2 = obj.getReference(2).?;
    try std.testing.expect(ref2.isNull());
}

test "Object: memory cleanup" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{1} ** 100;
    const var_data = [_]u8{2} ** 200;
    var refs: [10]ObjRef = undefined;
    for (&refs, 0..) |*ref, i| {
        ref.* = ObjRef.init(i * 10);
    }

    var obj = try Object.init(allocator, 1, id, &fixed, &var_data, &refs);
    obj.deinit();

    // If there's a memory leak, the test allocator will catch it
}

test "Object: serialization preserves header fields" {
    const allocator = std.testing.allocator;

    const id = [_]u8{1} ** 16;
    var obj = try Object.init(allocator, 42, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    obj.header.version = 5;
    obj.header.lsn = 12345;
    obj.header.setDeleted(true);

    const bytes = try obj.toBytes(allocator);
    defer allocator.free(bytes);

    var obj2 = try Object.fromBytes(allocator, bytes);
    defer obj2.deinit();

    try std.testing.expectEqual(@as(u32, 5), obj2.header.version);
    try std.testing.expectEqual(@as(u64, 12345), obj2.header.lsn);
    try std.testing.expect(obj2.header.isDeleted());
}
