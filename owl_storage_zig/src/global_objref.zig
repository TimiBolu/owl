// GlobalObjRef: 128-bit Distributed Pointers (Zig Implementation)
//
// This module implements GlobalObjRef in Zig, extending the current 64-bit ObjRef
// to support distributed object references across shards.
//
// Compatible with the Rust implementation in src/distributed/global_objref.rs

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;

/// Shard identifier type (32-bit unsigned integer)
pub const ShardId = u32;

/// 128-bit global object reference for distributed navigation
///
/// Memory Layout: [Shard ID: 32][Local Offset: 64][Flags: 32]
/// - shard_id: Which shard owns this object (0-4B shards supported)
/// - local_offset: Offset within shard's arena (same as ObjRef.offset)
/// - flags: Compression, type hints, version info, etc.
///
/// Design Goals:
/// - Backward compatible with existing ObjRef (local_offset field)
/// - O(1) navigation performance for local references (<50ns)
/// - Cross-shard navigation in <1μs per hop
/// - Efficient encoding for network transmission
/// - Exactly 16 bytes (128 bits) with packed layout
pub const GlobalObjRef = packed struct {
    /// Shard identifier (32 bits = 4 billion shards)
    shard_id: ShardId,
    /// Local offset within shard's arena (64 bits, same as ObjRef)
    local_offset: u64,
    /// Flags for metadata (32 bits)
    flags: u32,

    /// Flags for GlobalObjRef metadata
    pub const Flags = struct {
        /// Object is compressed with LZ4
        pub const COMPRESSED_LZ4: u32 = 0x0001;
        /// Object is compressed with ZSTD
        pub const COMPRESSED_ZSTD: u32 = 0x0002;
        /// Object is pinned in memory (hot data)
        pub const PINNED: u32 = 0x0004;
        /// Object has been migrated (stale reference)
        pub const MIGRATED: u32 = 0x0008;
        /// Object is a schema definition
        pub const SCHEMA: u32 = 0x0010;
        /// Object is an index entry
        pub const INDEX: u32 = 0x0020;
        /// Object is part of a transaction
        pub const TRANSACTIONAL: u32 = 0x0040;
        /// Object is replicated across multiple shards
        pub const REPLICATED: u32 = 0x0080;
    };

    /// Create a new GlobalObjRef
    pub inline fn init(shard_id: ShardId, local_offset: u64, flags: u32) GlobalObjRef {
        return .{
            .shard_id = shard_id,
            .local_offset = local_offset,
            .flags = flags,
        };
    }

    /// Create a GlobalObjRef from a local ObjRef
    pub inline fn fromLocal(local_ref: ObjRef, shard_id: ShardId) GlobalObjRef {
        return .{
            .shard_id = shard_id,
            .local_offset = local_ref.getOffset(),
            .flags = 0,
        };
    }

    /// Create a null GlobalObjRef
    pub inline fn nullRef() GlobalObjRef {
        return .{
            .shard_id = 0,
            .local_offset = 0,
            .flags = 0,
        };
    }

    /// Check if this is a null reference
    pub inline fn isNull(self: GlobalObjRef) bool {
        return self.local_offset == 0;
    }

    /// Check if reference is local to the current shard
    ///
    /// Returns true if object is on the current shard (O(1) navigation)
    /// Returns false if object is on a remote shard (network call required)
    pub inline fn isLocal(self: GlobalObjRef, current_shard: ShardId) bool {
        return self.shard_id == current_shard;
    }

    /// Convert to local ObjRef for same-shard navigation
    ///
    /// This enables O(1) navigation when the object is on the current shard.
    /// The returned ObjRef can be used with existing arena operations.
    pub inline fn toLocal(self: GlobalObjRef) ObjRef {
        return ObjRef.init(self.local_offset);
    }

    /// Get the shard ID
    pub inline fn getShardId(self: GlobalObjRef) ShardId {
        return self.shard_id;
    }

    /// Get the local offset
    pub inline fn getLocalOffset(self: GlobalObjRef) u64 {
        return self.local_offset;
    }

    /// Get the flags
    pub inline fn getFlags(self: GlobalObjRef) u32 {
        return self.flags;
    }

    /// Set a flag
    pub fn setFlag(self: *GlobalObjRef, flag: u32) void {
        self.flags |= flag;
    }

    /// Clear a flag
    pub fn clearFlag(self: *GlobalObjRef, flag: u32) void {
        self.flags &= ~flag;
    }

    /// Check if a flag is set
    pub inline fn hasFlag(self: GlobalObjRef, flag: u32) bool {
        return (self.flags & flag) != 0;
    }

    /// Check if object is compressed
    pub inline fn isCompressed(self: GlobalObjRef) bool {
        return self.hasFlag(Flags.COMPRESSED_LZ4) or self.hasFlag(Flags.COMPRESSED_ZSTD);
    }

    /// Check if object is pinned in memory
    pub inline fn isPinned(self: GlobalObjRef) bool {
        return self.hasFlag(Flags.PINNED);
    }

    /// Check if object has been migrated (stale reference)
    pub inline fn isMigrated(self: GlobalObjRef) bool {
        return self.hasFlag(Flags.MIGRATED);
    }

    /// Encode as 128-bit value for network transmission
    ///
    /// Layout: [shard_id: 32][local_offset: 64][flags: 32]
    /// Returns (high_64_bits, low_64_bits)
    pub fn encode(self: GlobalObjRef) struct { high: u64, low: u64 } {
        const high = (@as(u64, self.shard_id) << 32) | ((self.local_offset >> 32) & 0xFFFFFFFF);
        const low = ((self.local_offset & 0xFFFFFFFF) << 32) | @as(u64, self.flags);
        return .{ .high = high, .low = low };
    }

    /// Decode from 128-bit value
    pub fn decode(high: u64, low: u64) GlobalObjRef {
        const shard_id: u32 = @intCast(high >> 32);
        const local_offset = ((high & 0xFFFFFFFF) << 32) | (low >> 32);
        const flags: u32 = @intCast(low & 0xFFFFFFFF);

        return .{
            .shard_id = shard_id,
            .local_offset = local_offset,
            .flags = flags,
        };
    }

    /// Encode as bytes for storage (16 bytes, little-endian)
    pub fn toBytes(self: GlobalObjRef) [16]u8 {
        const encoded = self.encode();
        var bytes: [16]u8 = undefined;

        // Write high 64 bits (little-endian)
        std.mem.writeInt(u64, bytes[0..8], encoded.high, .little);
        // Write low 64 bits (little-endian)
        std.mem.writeInt(u64, bytes[8..16], encoded.low, .little);

        return bytes;
    }

    /// Decode from bytes (16 bytes, little-endian)
    pub fn fromBytes(bytes: []const u8) !GlobalObjRef {
        if (bytes.len != 16) {
            return error.InvalidLength;
        }

        const high = std.mem.readInt(u64, bytes[0..8], .little);
        const low = std.mem.readInt(u64, bytes[8..16], .little);

        return decode(high, low);
    }

    /// Get page ID for co-location (same as ObjRef)
    pub inline fn pageId(self: GlobalObjRef) u32 {
        return @intCast(self.local_offset / 4096);
    }

    /// Get slot ID within page (same as ObjRef)
    pub inline fn slotId(self: GlobalObjRef) u16 {
        return @intCast((self.local_offset % 4096) / 64);
    }

    /// Create a GlobalObjRef with updated shard (for migration)
    pub fn withShard(self: GlobalObjRef, new_shard_id: ShardId) GlobalObjRef {
        return .{
            .shard_id = new_shard_id,
            .local_offset = self.local_offset,
            .flags = self.flags,
        };
    }

    /// Create a GlobalObjRef with updated flags
    pub fn withFlags(self: GlobalObjRef, new_flags: u32) GlobalObjRef {
        return .{
            .shard_id = self.shard_id,
            .local_offset = self.local_offset,
            .flags = new_flags,
        };
    }

    /// Swizzle: Convert to memory pointer for local navigation
    ///
    /// This is the core operation for O(1) navigation.
    /// Only works for local references (same shard).
    ///
    /// # Safety
    /// The caller must ensure:
    /// - Reference is local to current shard
    /// - base pointer is valid
    /// - offset is within bounds
    /// - resulting pointer points to valid Object data
    pub inline fn swizzle(self: GlobalObjRef, base: [*]u8) [*]u8 {
        std.debug.assert(!self.isNull());
        return base + self.local_offset;
    }

    /// Format for printing (useful for debugging)
    pub fn format(
        self: GlobalObjRef,
        writer: anytype,
    ) !void {
        if (self.isNull()) {
            try writer.writeAll("GlobalObjRef(null)");
        } else {
            try writer.print("GlobalObjRef(shard={d}, offset={d}, flags=0x{x:0>8})", .{
                self.shard_id,
                self.local_offset,
                self.flags,
            });
        }
    }
};

/// Navigation result for cross-shard operations
pub fn NavigationResult(comptime T: type) type {
    return union(enum) {
        /// Object found locally (fast path)
        local: T,
        /// Object is on remote shard (requires network call)
        remote: struct {
            shard_id: ShardId,
            objref: GlobalObjRef,
        },
        /// Reference is null or invalid
        not_found,

        /// Check if result is local
        pub fn isLocal(self: @This()) bool {
            return switch (self) {
                .local => true,
                else => false,
            };
        }

        /// Check if result requires remote access
        pub fn isRemote(self: @This()) bool {
            return switch (self) {
                .remote => true,
                else => false,
            };
        }

        /// Get the local value if available
        pub fn getLocal(self: @This()) ?T {
            return switch (self) {
                .local => |value| value,
                else => null,
            };
        }

        /// Get remote shard info if applicable
        pub fn getRemoteInfo(self: @This()) ?@TypeOf(self.remote) {
            return switch (self) {
                .remote => |info| info,
                else => null,
            };
        }
    };
}

// Compile-time assertions to ensure correct size and alignment
comptime {
    // GlobalObjRef must be exactly 16 bytes (128 bits)
    if (@sizeOf(GlobalObjRef) != 16) {
        @compileError("GlobalObjRef must be exactly 16 bytes");
    }

    // Packed struct alignment - should be reasonable for the largest field (u64)
    if (@alignOf(GlobalObjRef) > 16) {
        @compileError("GlobalObjRef alignment is too large");
    }
}

// ============================================================================
// Tests
// ============================================================================

test "GlobalObjRef: size is 16 bytes" {
    try std.testing.expectEqual(@as(usize, 16), @sizeOf(GlobalObjRef));
}

test "GlobalObjRef: alignment is reasonable" {
    try std.testing.expect(@alignOf(GlobalObjRef) <= 16);
}

test "GlobalObjRef: creation" {
    const global_ref = GlobalObjRef.init(1, 1024, 0);
    try std.testing.expectEqual(@as(ShardId, 1), global_ref.getShardId());
    try std.testing.expectEqual(@as(u64, 1024), global_ref.getLocalOffset());
    try std.testing.expectEqual(@as(u32, 0), global_ref.getFlags());
    try std.testing.expect(!global_ref.isNull());
}

test "GlobalObjRef: null reference" {
    const null_ref = GlobalObjRef.nullRef();
    try std.testing.expect(null_ref.isNull());
    try std.testing.expectEqual(@as(ShardId, 0), null_ref.getShardId());
    try std.testing.expectEqual(@as(u64, 0), null_ref.getLocalOffset());
}

test "GlobalObjRef: local conversion" {
    const local_ref = ObjRef.init(2048);
    const global_ref = GlobalObjRef.fromLocal(local_ref, 5);

    try std.testing.expectEqual(@as(ShardId, 5), global_ref.getShardId());
    try std.testing.expectEqual(@as(u64, 2048), global_ref.getLocalOffset());
    try std.testing.expect(global_ref.isLocal(5));
    try std.testing.expect(!global_ref.isLocal(3));

    const local_again = global_ref.toLocal();
    try std.testing.expectEqual(@as(u64, 2048), local_again.getOffset());
}

test "GlobalObjRef: flags" {
    var global_ref = GlobalObjRef.init(1, 1024, 0);

    // Set compression flag
    global_ref.setFlag(GlobalObjRef.Flags.COMPRESSED_LZ4);
    try std.testing.expect(global_ref.hasFlag(GlobalObjRef.Flags.COMPRESSED_LZ4));
    try std.testing.expect(global_ref.isCompressed());

    // Set pinned flag
    global_ref.setFlag(GlobalObjRef.Flags.PINNED);
    try std.testing.expect(global_ref.hasFlag(GlobalObjRef.Flags.PINNED));
    try std.testing.expect(global_ref.isPinned());

    // Clear compression flag
    global_ref.clearFlag(GlobalObjRef.Flags.COMPRESSED_LZ4);
    try std.testing.expect(!global_ref.hasFlag(GlobalObjRef.Flags.COMPRESSED_LZ4));
    try std.testing.expect(!global_ref.isCompressed());
    try std.testing.expect(global_ref.isPinned()); // Should still be pinned
}

test "GlobalObjRef: encoding and decoding" {
    const original = GlobalObjRef.init(0x12345678, 0xABCDEF0123456789, 0x87654321);

    const encoded = original.encode();
    const decoded = GlobalObjRef.decode(encoded.high, encoded.low);

    try std.testing.expectEqual(original.shard_id, decoded.shard_id);
    try std.testing.expectEqual(original.local_offset, decoded.local_offset);
    try std.testing.expectEqual(original.flags, decoded.flags);
}

test "GlobalObjRef: bytes serialization" {
    const original = GlobalObjRef.init(42, 1024 * 1024, GlobalObjRef.Flags.COMPRESSED_ZSTD | GlobalObjRef.Flags.PINNED);

    const bytes = original.toBytes();
    try std.testing.expectEqual(@as(usize, 16), bytes.len);

    const decoded = try GlobalObjRef.fromBytes(&bytes);
    try std.testing.expectEqual(original.shard_id, decoded.shard_id);
    try std.testing.expectEqual(original.local_offset, decoded.local_offset);
    try std.testing.expectEqual(original.flags, decoded.flags);
}

test "GlobalObjRef: bytes invalid length" {
    const short_bytes = [_]u8{0} ** 8;
    const result = GlobalObjRef.fromBytes(&short_bytes);
    try std.testing.expectError(error.InvalidLength, result);
}

test "GlobalObjRef: page and slot" {
    const global_ref = GlobalObjRef.init(1, 4096 + 128, 0);

    try std.testing.expectEqual(@as(u32, 1), global_ref.pageId());
    try std.testing.expectEqual(@as(u16, 2), global_ref.slotId()); // 128 / 64 = 2
}

test "GlobalObjRef: with methods" {
    const original = GlobalObjRef.init(1, 1024, GlobalObjRef.Flags.PINNED);

    const with_new_shard = original.withShard(5);
    try std.testing.expectEqual(@as(ShardId, 5), with_new_shard.getShardId());
    try std.testing.expectEqual(@as(u64, 1024), with_new_shard.getLocalOffset());
    try std.testing.expectEqual(GlobalObjRef.Flags.PINNED, with_new_shard.getFlags());

    const with_new_flags = original.withFlags(GlobalObjRef.Flags.COMPRESSED_LZ4);
    try std.testing.expectEqual(@as(ShardId, 1), with_new_flags.getShardId());
    try std.testing.expectEqual(@as(u64, 1024), with_new_flags.getLocalOffset());
    try std.testing.expectEqual(GlobalObjRef.Flags.COMPRESSED_LZ4, with_new_flags.getFlags());
}

test "GlobalObjRef: swizzling" {
    const base_addr: usize = 0x1000000;
    const base: [*]u8 = @ptrFromInt(base_addr);

    const global_ref = GlobalObjRef.init(0, 4096 + 128, 0);
    const ptr = global_ref.swizzle(base);

    const expected_addr = base_addr + 4096 + 128;
    try std.testing.expectEqual(expected_addr, @intFromPtr(ptr));
}

test "GlobalObjRef: compression flags" {
    var global_ref = GlobalObjRef.init(1, 1024, 0);

    // No compression initially
    try std.testing.expect(!global_ref.isCompressed());

    // LZ4 compression
    global_ref.setFlag(GlobalObjRef.Flags.COMPRESSED_LZ4);
    try std.testing.expect(global_ref.isCompressed());

    // Clear LZ4, set ZSTD
    global_ref.clearFlag(GlobalObjRef.Flags.COMPRESSED_LZ4);
    global_ref.setFlag(GlobalObjRef.Flags.COMPRESSED_ZSTD);
    try std.testing.expect(global_ref.isCompressed());

    // Clear all compression
    global_ref.clearFlag(GlobalObjRef.Flags.COMPRESSED_ZSTD);
    try std.testing.expect(!global_ref.isCompressed());
}

test "GlobalObjRef: migration flag" {
    var global_ref = GlobalObjRef.init(1, 1024, 0);

    try std.testing.expect(!global_ref.isMigrated());

    global_ref.setFlag(GlobalObjRef.Flags.MIGRATED);
    try std.testing.expect(global_ref.isMigrated());
}

test "GlobalObjRef: large values" {
    // Test with maximum values
    const max_shard = std.math.maxInt(u32);
    const max_offset = std.math.maxInt(u64);
    const max_flags = std.math.maxInt(u32);

    const global_ref = GlobalObjRef.init(max_shard, max_offset, max_flags);

    const encoded = global_ref.encode();
    const decoded = GlobalObjRef.decode(encoded.high, encoded.low);

    try std.testing.expectEqual(global_ref.shard_id, decoded.shard_id);
    try std.testing.expectEqual(global_ref.local_offset, decoded.local_offset);
    try std.testing.expectEqual(global_ref.flags, decoded.flags);

    const bytes = global_ref.toBytes();
    const from_bytes = try GlobalObjRef.fromBytes(&bytes);
    try std.testing.expectEqual(global_ref.shard_id, from_bytes.shard_id);
    try std.testing.expectEqual(global_ref.local_offset, from_bytes.local_offset);
    try std.testing.expectEqual(global_ref.flags, from_bytes.flags);
}

test "NavigationResult: local result" {
    const local_data = "local_object";
    const local_result = NavigationResult([]const u8){ .local = local_data };

    try std.testing.expect(local_result.isLocal());
    try std.testing.expect(!local_result.isRemote());
    try std.testing.expectEqualStrings("local_object", local_result.getLocal().?);
}

test "NavigationResult: remote result" {
    const global_ref = GlobalObjRef.init(5, 1024, 0);
    const remote_result = NavigationResult([]const u8){ .remote = .{
        .shard_id = 5,
        .objref = global_ref,
    } };

    try std.testing.expect(!remote_result.isLocal());
    try std.testing.expect(remote_result.isRemote());

    const remote_info = remote_result.getRemoteInfo().?;
    try std.testing.expectEqual(@as(ShardId, 5), remote_info.shard_id);
    try std.testing.expectEqual(global_ref.shard_id, remote_info.objref.shard_id);
}

test "NavigationResult: not found result" {
    const not_found = NavigationResult([]const u8){ .not_found = {} };
    try std.testing.expect(!not_found.isLocal());
    try std.testing.expect(!not_found.isRemote());
    try std.testing.expect(not_found.getLocal() == null);
    try std.testing.expect(not_found.getRemoteInfo() == null);
}

test "GlobalObjRef: format output" {
    const global_ref = GlobalObjRef.init(1, 1024, 0x12345678);
    var buf: [200]u8 = undefined;
    const result = try std.fmt.bufPrint(&buf, "{f}", .{global_ref});

    try std.testing.expect(std.mem.indexOf(u8, result, "GlobalObjRef") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "shard=1") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "offset=1024") != null);
    try std.testing.expect(std.mem.indexOf(u8, result, "flags=0x12345678") != null);
}

test "GlobalObjRef: null format output" {
    const null_ref = GlobalObjRef.nullRef();
    var buf: [100]u8 = undefined;
    const result = try std.fmt.bufPrint(&buf, "{f}", .{null_ref});

    try std.testing.expect(std.mem.indexOf(u8, result, "null") != null);
}
