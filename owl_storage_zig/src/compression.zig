// Task 10: Adaptive Compression
// Implements LZ4 and ZSTD compression with size-based selection
// Uses C libraries: liblz4 and libzstd

const std = @import("std");
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const c = @cImport({
    @cInclude("lz4.h");
    @cInclude("lz4hc.h");
    @cInclude("zstd.h");
});

/// Compression type enumeration (2 bits = 4 types)
pub const CompressionType = enum(u2) {
    none = 0,
    lz4 = 1,
    zstd = 2,
    reserved = 3, // Reserved for future use

    /// Get compression type from flags
    pub fn fromFlags(flags: u8) CompressionType {
        const compression_bits = (flags >> 3) & 0x03; // Bits 3-4
        return @enumFromInt(compression_bits);
    }

    /// Set compression type in flags
    pub fn toFlags(self: CompressionType, flags: u8) u8 {
        const cleared = flags & ~@as(u8, 0x18); // Clear bits 3-4
        const compression_bits: u8 = @intFromEnum(self);
        return cleared | (compression_bits << 3);
    }
};

/// Compression thresholds based on requirements
pub const COMPRESSION_THRESHOLD_MIN: u32 = 1024; // 1KB - don't compress smaller objects
pub const LZ4_THRESHOLD_MAX: u32 = 4096; // 4KB - use LZ4 for objects < 4KB
pub const ZSTD_THRESHOLD: u32 = 4096; // 4KB - use ZSTD for objects >= 4KB

/// Compression statistics
pub const CompressionStats = struct {
    compressed_bytes: std.atomic.Value(u64),
    uncompressed_bytes: std.atomic.Value(u64),
    compression_count: std.atomic.Value(u64),
    decompression_count: std.atomic.Value(u64),
    lz4_count: std.atomic.Value(u64),
    zstd_count: std.atomic.Value(u64),

    pub fn init() CompressionStats {
        return .{
            .compressed_bytes = std.atomic.Value(u64).init(0),
            .uncompressed_bytes = std.atomic.Value(u64).init(0),
            .compression_count = std.atomic.Value(u64).init(0),
            .decompression_count = std.atomic.Value(u64).init(0),
            .lz4_count = std.atomic.Value(u64).init(0),
            .zstd_count = std.atomic.Value(u64).init(0),
        };
    }

    /// Calculate compression ratio
    pub fn compressionRatio(self: *const CompressionStats) f64 {
        const uncompressed = self.uncompressed_bytes.load(.seq_cst);
        if (uncompressed == 0) return 1.0;

        const compressed = self.compressed_bytes.load(.seq_cst);
        return @as(f64, @floatFromInt(compressed)) / @as(f64, @floatFromInt(uncompressed));
    }

    /// Calculate space savings percentage
    pub fn spaceSavings(self: *const CompressionStats) f64 {
        return (1.0 - self.compressionRatio()) * 100.0;
    }
};

/// Compressed data header
pub const CompressedHeader = extern struct {
    uncompressed_size: u32,
    compressed_size: u32,
    compression_type: u8,
    _padding: [3]u8,

    pub fn init(uncompressed_size: u32, compressed_size: u32, compression_type: CompressionType) CompressedHeader {
        return .{
            .uncompressed_size = uncompressed_size,
            .compressed_size = compressed_size,
            .compression_type = @intFromEnum(compression_type),
            ._padding = [_]u8{0} ** 3,
        };
    }

    pub fn headerSize() usize {
        return @sizeOf(CompressedHeader);
    }
};

/// Compression engine with adaptive selection
pub const CompressionEngine = struct {
    allocator: std.mem.Allocator,
    stats: CompressionStats,

    pub fn init(allocator: std.mem.Allocator) CompressionEngine {
        return .{
            .allocator = allocator,
            .stats = CompressionStats.init(),
        };
    }

    /// Select compression type based on data size
    pub fn selectCompressionType(size: u32) CompressionType {
        if (size < COMPRESSION_THRESHOLD_MIN) {
            return .none; // Too small to compress
        } else if (size < LZ4_THRESHOLD_MAX) {
            return .lz4; // Fast compression for small-medium objects
        } else {
            return .zstd; // High compression ratio for large objects
        }
    }

    /// Compress data with adaptive selection
    pub fn compress(self: *CompressionEngine, data: []const u8) ![]u8 {
        const compression_type = selectCompressionType(@intCast(data.len));

        if (compression_type == .none) {
            // No compression - just copy data
            const result = try self.allocator.dupe(u8, data);
            return result;
        }

        _ = self.stats.compression_count.fetchAdd(1, .seq_cst);
        _ = self.stats.uncompressed_bytes.fetchAdd(data.len, .seq_cst);

        const compressed = switch (compression_type) {
            .lz4 => try self.compressLZ4(data),
            .zstd => try self.compressZSTD(data),
            .none, .reserved => unreachable,
        };

        _ = self.stats.compressed_bytes.fetchAdd(compressed.len, .seq_cst);

        switch (compression_type) {
            .lz4 => _ = self.stats.lz4_count.fetchAdd(1, .seq_cst),
            .zstd => _ = self.stats.zstd_count.fetchAdd(1, .seq_cst),
            .none, .reserved => {},
        }

        return compressed;
    }

    /// Compress data with specific type
    pub fn compressWithType(self: *CompressionEngine, data: []const u8, compression_type: CompressionType) ![]u8 {
        if (compression_type == .none) {
            return try self.allocator.dupe(u8, data);
        }

        _ = self.stats.compression_count.fetchAdd(1, .seq_cst);
        _ = self.stats.uncompressed_bytes.fetchAdd(data.len, .seq_cst);

        const compressed = switch (compression_type) {
            .lz4 => try self.compressLZ4(data),
            .zstd => try self.compressZSTD(data),
            .none, .reserved => unreachable,
        };

        _ = self.stats.compressed_bytes.fetchAdd(compressed.len, .seq_cst);

        switch (compression_type) {
            .lz4 => _ = self.stats.lz4_count.fetchAdd(1, .seq_cst),
            .zstd => _ = self.stats.zstd_count.fetchAdd(1, .seq_cst),
            .none, .reserved => {},
        }

        return compressed;
    }

    /// Decompress data
    pub fn decompress(self: *CompressionEngine, data: []const u8, compression_type: CompressionType) ![]u8 {
        if (compression_type == .none) {
            return try self.allocator.dupe(u8, data);
        }

        _ = self.stats.decompression_count.fetchAdd(1, .seq_cst);

        return switch (compression_type) {
            .lz4 => try self.decompressLZ4(data),
            .zstd => try self.decompressZSTD(data),
            .none, .reserved => error.InvalidCompressionType,
        };
    }

    /// LZ4 compression (fast, moderate ratio)
    /// Uses liblz4 C library
    fn compressLZ4(self: *CompressionEngine, data: []const u8) ![]u8 {
        // Get maximum compressed size
        const max_compressed_size: usize = @intCast(c.LZ4_compressBound(@intCast(data.len)));

        // Allocate buffer for header + compressed data
        var compressed_buffer = try self.allocator.alloc(u8, max_compressed_size + CompressedHeader.headerSize());
        errdefer self.allocator.free(compressed_buffer);

        // Compress using LZ4
        const compressed_size = c.LZ4_compress_default(
            data.ptr,
            compressed_buffer.ptr + CompressedHeader.headerSize(),
            @intCast(data.len),
            @intCast(max_compressed_size),
        );

        if (compressed_size <= 0) {
            return error.CompressionFailed;
        }

        // Add header
        const header = CompressedHeader.init(
            @intCast(data.len),
            @intCast(compressed_size),
            .lz4,
        );
        @memcpy(compressed_buffer[0..CompressedHeader.headerSize()], std.mem.asBytes(&header));

        // Resize to actual size
        const total_size = CompressedHeader.headerSize() + @as(usize, @intCast(compressed_size));
        const result = try self.allocator.realloc(compressed_buffer, total_size);
        return result;
    }

    /// LZ4 decompression
    fn decompressLZ4(self: *CompressionEngine, data: []const u8) ![]u8 {
        if (data.len < CompressedHeader.headerSize()) {
            return error.InvalidCompressedData;
        }

        // Read header
        const header: *const CompressedHeader = @ptrCast(@alignCast(data.ptr));
        const compressed_data = data[CompressedHeader.headerSize()..];

        // Allocate output buffer
        const output = try self.allocator.alloc(u8, header.uncompressed_size);
        errdefer self.allocator.free(output);

        // Decompress using LZ4
        const decompressed_size = c.LZ4_decompress_safe(
            compressed_data.ptr,
            output.ptr,
            @intCast(compressed_data.len),
            @intCast(header.uncompressed_size),
        );

        if (decompressed_size < 0) {
            return error.DecompressionFailed;
        }

        if (@as(u32, @intCast(decompressed_size)) != header.uncompressed_size) {
            return error.SizeMismatch;
        }

        return output;
    }

    /// ZSTD compression (slower, high ratio)
    /// Uses libzstd C library
    fn compressZSTD(self: *CompressionEngine, data: []const u8) ![]u8 {
        // Get maximum compressed size
        const max_compressed_size: usize = @intCast(c.ZSTD_compressBound(@intCast(data.len)));

        // Allocate buffer for header + compressed data
        var compressed_buffer = try self.allocator.alloc(u8, max_compressed_size + CompressedHeader.headerSize());
        errdefer self.allocator.free(compressed_buffer);

        // Compress using ZSTD with default compression level (3)
        const compressed_size = c.ZSTD_compress(
            compressed_buffer.ptr + CompressedHeader.headerSize(),
            max_compressed_size,
            data.ptr,
            data.len,
            3, // Compression level (1-22, 3 is default)
        );

        if (c.ZSTD_isError(compressed_size) != 0) {
            return error.CompressionFailed;
        }

        // Add header
        const header = CompressedHeader.init(
            @intCast(data.len),
            @intCast(compressed_size),
            .zstd,
        );
        @memcpy(compressed_buffer[0..CompressedHeader.headerSize()], std.mem.asBytes(&header));

        // Resize to actual size
        const total_size = CompressedHeader.headerSize() + compressed_size;
        const result = try self.allocator.realloc(compressed_buffer, total_size);
        return result;
    }

    /// ZSTD decompression
    fn decompressZSTD(self: *CompressionEngine, data: []const u8) ![]u8 {
        if (data.len < CompressedHeader.headerSize()) {
            return error.InvalidCompressedData;
        }

        // Read header
        const header: *const CompressedHeader = @ptrCast(@alignCast(data.ptr));
        const compressed_data = data[CompressedHeader.headerSize()..];

        // Allocate output buffer
        const output = try self.allocator.alloc(u8, header.uncompressed_size);
        errdefer self.allocator.free(output);

        // Decompress using ZSTD
        const decompressed_size = c.ZSTD_decompress(
            output.ptr,
            header.uncompressed_size,
            compressed_data.ptr,
            compressed_data.len,
        );

        if (c.ZSTD_isError(decompressed_size) != 0) {
            return error.DecompressionFailed;
        }

        if (decompressed_size != header.uncompressed_size) {
            return error.SizeMismatch;
        }

        return output;
    }

    /// Get compression statistics
    pub fn getStats(self: *const CompressionEngine) *const CompressionStats {
        return &self.stats;
    }
};

// ============================================================================
// Tests
// ============================================================================

test "CompressionType: from/to flags" {
    var flags: u8 = 0;

    // Test none
    flags = CompressionType.none.toFlags(flags);
    try std.testing.expectEqual(CompressionType.none, CompressionType.fromFlags(flags));

    // Test LZ4
    flags = CompressionType.lz4.toFlags(flags);
    try std.testing.expectEqual(CompressionType.lz4, CompressionType.fromFlags(flags));

    // Test ZSTD
    flags = CompressionType.zstd.toFlags(flags);
    try std.testing.expectEqual(CompressionType.zstd, CompressionType.fromFlags(flags));
}

test "CompressionEngine: select compression type" {
    // Small objects - no compression
    try std.testing.expectEqual(CompressionType.none, CompressionEngine.selectCompressionType(512));
    try std.testing.expectEqual(CompressionType.none, CompressionEngine.selectCompressionType(1023));

    // Medium objects - LZ4
    try std.testing.expectEqual(CompressionType.lz4, CompressionEngine.selectCompressionType(1024));
    try std.testing.expectEqual(CompressionType.lz4, CompressionEngine.selectCompressionType(2048));
    try std.testing.expectEqual(CompressionType.lz4, CompressionEngine.selectCompressionType(4095));

    // Large objects - ZSTD
    try std.testing.expectEqual(CompressionType.zstd, CompressionEngine.selectCompressionType(4096));
    try std.testing.expectEqual(CompressionType.zstd, CompressionEngine.selectCompressionType(1024 * 1024));
}

test "CompressionEngine: LZ4 round-trip" {
    const allocator = std.testing.allocator;
    var engine = CompressionEngine.init(allocator);

    // Test data with runs
    const data = "AAAAAABBBBBBCCCCCCDDDDDD";

    const compressed = try engine.compressWithType(data, .lz4);
    defer allocator.free(compressed);

    try std.testing.expect(compressed.len > 0);

    const decompressed = try engine.decompress(compressed, .lz4);
    defer allocator.free(decompressed);

    try std.testing.expectEqualStrings(data, decompressed);
}

test "CompressionEngine: ZSTD round-trip" {
    const allocator = std.testing.allocator;
    var engine = CompressionEngine.init(allocator);

    const data = "The quick brown fox jumps over the lazy dog";

    const compressed = try engine.compressWithType(data, .zstd);
    defer allocator.free(compressed);

    try std.testing.expect(compressed.len > 0);

    const decompressed = try engine.decompress(compressed, .zstd);
    defer allocator.free(decompressed);

    try std.testing.expectEqualStrings(data, decompressed);
}

test "CompressionEngine: adaptive compression" {
    const allocator = std.testing.allocator;
    var engine = CompressionEngine.init(allocator);

    // Small data - no compression
    const small_data = "Hello";
    const small_compressed = try engine.compress(small_data);
    defer allocator.free(small_compressed);
    try std.testing.expectEqualStrings(small_data, small_compressed);

    // Medium data - LZ4
    const medium_data = [_]u8{0xAA} ** 2048;
    const medium_compressed = try engine.compress(&medium_data);
    defer allocator.free(medium_compressed);
    try std.testing.expect(medium_compressed.len < medium_data.len);

    // Large data - ZSTD
    const large_data = [_]u8{0xBB} ** 8192;
    const large_compressed = try engine.compress(&large_data);
    defer allocator.free(large_compressed);
    try std.testing.expect(large_compressed.len < large_data.len);
}

test "CompressionEngine: statistics" {
    const allocator = std.testing.allocator;
    var engine = CompressionEngine.init(allocator);

    const data = [_]u8{0xAA} ** 2048;

    const compressed = try engine.compress(&data);
    defer allocator.free(compressed);

    const decompressed = try engine.decompress(compressed, .lz4);
    defer allocator.free(decompressed);

    const stats = engine.getStats();
    try std.testing.expect(stats.compression_count.load(.seq_cst) == 1);
    try std.testing.expect(stats.decompression_count.load(.seq_cst) == 1);
    try std.testing.expect(stats.uncompressed_bytes.load(.seq_cst) == 2048);
}

test "CompressionStats: compression ratio" {
    var stats = CompressionStats.init();

    _ = stats.uncompressed_bytes.fetchAdd(1000, .seq_cst);
    _ = stats.compressed_bytes.fetchAdd(500, .seq_cst);

    const ratio = stats.compressionRatio();
    try std.testing.expectApproxEqAbs(@as(f64, 0.5), ratio, 0.01);

    const savings = stats.spaceSavings();
    try std.testing.expectApproxEqAbs(@as(f64, 50.0), savings, 0.1);
}
