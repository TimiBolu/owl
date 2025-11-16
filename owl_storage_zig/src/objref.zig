// Task 1.1: ObjRef Implementation
// Physical pointer represented as a 64-bit offset into the storage file
// This enables O(1) navigation through object references

const std = @import("std");

/// Physical pointer represented as a 64-bit offset into the storage file.
/// This enables O(1) navigation through object references.
///
/// Memory Layout: Simple 64-bit offset (8 bytes total)
/// - offset: u64 (file-global offset)
///
/// Design: Uses `extern struct` for C ABI compatibility
pub const ObjRef = extern struct {
    /// File-global offset (8 bytes)
    offset: u64,

    /// Create a new object reference from an offset
    pub inline fn init(offset: u64) ObjRef {
        return .{ .offset = offset };
    }

    /// Create a null reference (offset = 0)
    pub inline fn nullRef() ObjRef {
        return .{ .offset = 0 };
    }

    /// Check if this is a null reference
    pub inline fn isNull(self: ObjRef) bool {
        return self.offset == 0;
    }

    /// Get the raw offset value
    pub inline fn getOffset(self: ObjRef) u64 {
        return self.offset;
    }

    /// Swizzle: Convert disk offset to memory pointer
    ///
    /// This is the core operation for O(1) navigation.
    /// Given a base pointer to the memory-mapped region,
    /// returns a pointer to the object at this offset.
    ///
    /// # Safety
    /// The caller must ensure:
    /// - base pointer is valid
    /// - offset is within bounds
    /// - resulting pointer points to valid Object data
    pub inline fn swizzle(self: ObjRef, base: [*]u8) [*]u8 {
        return base + self.offset;
    }

    /// Get the page ID (4KB page number) for this reference.
    /// Used for co-location and cache optimization.
    ///
    /// Page size is 4096 bytes (4KB)
    pub inline fn pageId(self: ObjRef) u32 {
        return @intCast(self.offset / 4096);
    }

    /// Get the slot ID within the page.
    /// Assumes 64-byte slots within each 4KB page.
    ///
    /// Slot size is 64 bytes, giving 64 slots per page.
    pub inline fn slotId(self: ObjRef) u16 {
        return @intCast((self.offset % 4096) / 64);
    }

    /// Create an ObjRef from page_id and slot_id
    pub inline fn fromPageSlot(page_id: u32, slot_id: u16) ObjRef {
        const offset = (@as(u64, page_id) * 4096) + (@as(u64, slot_id) * 64);
        return .{ .offset = offset };
    }

    /// Format for printing (useful for debugging)
    pub fn format(
        self: ObjRef,
        writer: anytype,
    ) !void {
        if (self.isNull()) {
            try writer.writeAll("ObjRef(null)");
        } else {
            try writer.print("ObjRef(offset={d}, page={d}, slot={d})", .{
                self.offset,
                self.pageId(),
                self.slotId(),
            });
        }
    }
};

// Compile-time assertions to ensure correct size and alignment
comptime {
    // ObjRef must be exactly 8 bytes for C ABI compatibility
    if (@sizeOf(ObjRef) != 8) {
        @compileError("ObjRef must be exactly 8 bytes");
    }

    // ObjRef must be properly aligned
    if (@alignOf(ObjRef) != 8) {
        @compileError("ObjRef must be 8-byte aligned");
    }
}

// ============================================================================
// Tests
// ============================================================================

test "ObjRef: size is 8 bytes" {
    try std.testing.expectEqual(@as(usize, 8), @sizeOf(ObjRef));
}

test "ObjRef: alignment is 8 bytes" {
    try std.testing.expectEqual(@as(usize, 8), @alignOf(ObjRef));
}

test "ObjRef: null reference" {
    const null_ref = ObjRef.nullRef();
    try std.testing.expect(null_ref.isNull());
    try std.testing.expectEqual(@as(u64, 0), null_ref.getOffset());
}

test "ObjRef: non-null reference" {
    const ref = ObjRef.init(1024);
    try std.testing.expect(!ref.isNull());
    try std.testing.expectEqual(@as(u64, 1024), ref.getOffset());
}

test "ObjRef: page and slot calculation" {
    // Test page 0, slot 0
    const ref0 = ObjRef.init(0);
    try std.testing.expectEqual(@as(u32, 0), ref0.pageId());
    try std.testing.expectEqual(@as(u16, 0), ref0.slotId());

    // Test page 1, slot 2 (offset = 4096 + 128)
    const ref1 = ObjRef.init(4096 + 128);
    try std.testing.expectEqual(@as(u32, 1), ref1.pageId());
    try std.testing.expectEqual(@as(u16, 2), ref1.slotId());

    // Test last slot of page 0 (offset = 4096 - 64)
    const ref_last = ObjRef.init(4096 - 64);
    try std.testing.expectEqual(@as(u32, 0), ref_last.pageId());
    try std.testing.expectEqual(@as(u16, 63), ref_last.slotId());

    // Test first slot of page 1
    const ref_page1 = ObjRef.init(4096);
    try std.testing.expectEqual(@as(u32, 1), ref_page1.pageId());
    try std.testing.expectEqual(@as(u16, 0), ref_page1.slotId());
}

test "ObjRef: fromPageSlot reconstruction" {
    const page_id: u32 = 5;
    const slot_id: u16 = 10;

    const ref = ObjRef.fromPageSlot(page_id, slot_id);
    try std.testing.expectEqual(page_id, ref.pageId());
    try std.testing.expectEqual(slot_id, ref.slotId());

    const expected_offset = (@as(u64, page_id) * 4096) + (@as(u64, slot_id) * 64);
    try std.testing.expectEqual(expected_offset, ref.getOffset());
}

test "ObjRef: swizzling" {
    const base_addr: usize = 0x1000000;
    const base: [*]u8 = @ptrFromInt(base_addr);

    const ref = ObjRef.init(4096 + 128);
    const ptr = ref.swizzle(base);

    const expected_addr = base_addr + 4096 + 128;
    try std.testing.expectEqual(expected_addr, @intFromPtr(ptr));
}

test "ObjRef: page boundaries" {
    // Test various page boundaries
    const test_cases = [_]struct {
        offset: u64,
        expected_page: u32,
        expected_slot: u16,
    }{
        .{ .offset = 0, .expected_page = 0, .expected_slot = 0 },
        .{ .offset = 64, .expected_page = 0, .expected_slot = 1 },
        .{ .offset = 128, .expected_page = 0, .expected_slot = 2 },
        .{ .offset = 4032, .expected_page = 0, .expected_slot = 63 },
        .{ .offset = 4096, .expected_page = 1, .expected_slot = 0 },
        .{ .offset = 8192, .expected_page = 2, .expected_slot = 0 },
        .{ .offset = 8256, .expected_page = 2, .expected_slot = 1 },
    };

    for (test_cases) |tc| {
        const ref = ObjRef.init(tc.offset);
        try std.testing.expectEqual(tc.expected_page, ref.pageId());
        try std.testing.expectEqual(tc.expected_slot, ref.slotId());
    }
}

test "ObjRef: equality" {
    const ref1 = ObjRef.init(1024);
    const ref2 = ObjRef.init(1024);
    const ref3 = ObjRef.init(2048);

    try std.testing.expectEqual(ref1.offset, ref2.offset);
    try std.testing.expect(ref1.offset != ref3.offset);
}

test "ObjRef: null vs non-null" {
    const null_ref = ObjRef.nullRef();
    const valid_ref = ObjRef.init(64);

    try std.testing.expect(null_ref.isNull());
    try std.testing.expect(!valid_ref.isNull());
    try std.testing.expect(null_ref.offset != valid_ref.offset);
}

test "ObjRef: large offsets" {
    // Test with large offsets (multi-GB files)
    const large_offset: u64 = 1024 * 1024 * 1024; // 1GB
    const ref = ObjRef.init(large_offset);

    try std.testing.expectEqual(large_offset, ref.getOffset());
    try std.testing.expect(!ref.isNull());

    const expected_page = @as(u32, @intCast(large_offset / 4096));
    try std.testing.expectEqual(expected_page, ref.pageId());
}

test "ObjRef: format output" {
    const ref = ObjRef.init(4096 + 128);
    var buf: [100]u8 = undefined;
    const result = try std.fmt.bufPrint(&buf, "{f}", .{ref});

    try std.testing.expect(std.mem.indexOf(u8, result, "ObjRef") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "4224") != null); // offset
    try std.testing.expect(std.mem.indexOf(u8, result, "page=1") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "slot=2") != null);
}

test "ObjRef: null format output" {
    const null_ref = ObjRef.nullRef();
    var buf: [100]u8 = undefined;
    const result = try std.fmt.bufPrint(&buf, "{f}", .{null_ref});

    try std.testing.expect(std.mem.indexOf(u8, result, "null") != null);
}
