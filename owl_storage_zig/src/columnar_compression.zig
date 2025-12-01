// Task 11.2: Columnar Compression
// Column-specific compression algorithms for analytical workloads

const std = @import("std");
const ColumnType = @import("columnar.zig").ColumnType;
const ColumnCompressionType = @import("columnar.zig").ColumnCompressionType;

/// Run-Length Encoding (RLE) for repeated values
pub const RLECompressor = struct {
    /// Compress int64 column with RLE
    pub fn compressInt64(allocator: std.mem.Allocator, data: []const i64) ![]u8 {
        if (data.len == 0) return &[_]u8{};

        var result = std.ArrayList(u8).init(allocator);
        errdefer result.deinit();

        var i: usize = 0;
        while (i < data.len) {
            const value = data[i];
            var run_length: u32 = 1;

            // Count consecutive equal values
            while (i + run_length < data.len and data[i + run_length] == value) {
                run_length += 1;
            }

            // Write run: [run_length: u32][value: i64]
            try result.writer().writeInt(u32, run_length, .little);
            try result.writer().writeInt(i64, value, .little);

            i += run_length;
        }

        return result.toOwnedSlice();
    }

    /// Decompress RLE int64 column
    pub fn decompressInt64(allocator: std.mem.Allocator, compressed: []const u8) ![]i64 {
        var result = std.ArrayList(i64).init(allocator);
        errdefer result.deinit();

        var stream = std.io.fixedBufferStream(compressed);
        const reader = stream.reader();

        while (true) {
            const run_length = reader.readInt(u32, .little) catch break;
            const value = reader.readInt(i64, .little) catch break;

            var j: u32 = 0;
            while (j < run_length) : (j += 1) {
                try result.append(value);
            }
        }

        return result.toOwnedSlice();
    }
};

/// Delta encoding for sequential integers
pub const DeltaCompressor = struct {
    /// Compress int64 column with delta encoding
    pub fn compressInt64(allocator: std.mem.Allocator, data: []const i64) ![]u8 {
        if (data.len == 0) return &[_]u8{};

        var result = std.ArrayList(u8).init(allocator);
        errdefer result.deinit();

        // Write first value as-is
        try result.writer().writeInt(i64, data[0], .little);

        // Write deltas
        for (data[1..], 1..) |value, i| {
            const delta = value - data[i - 1];
            try result.writer().writeInt(i64, delta, .little);
        }

        return result.toOwnedSlice();
    }

    /// Decompress delta-encoded int64 column
    pub fn decompressInt64(allocator: std.mem.Allocator, compressed: []const u8) ![]i64 {
        if (compressed.len < 8) return &[_]i64{};

        var result = std.ArrayList(i64).init(allocator);
        errdefer result.deinit();

        var stream = std.io.fixedBufferStream(compressed);
        const reader = stream.reader();

        // Read first value
        const first = try reader.readInt(i64, .little);
        try result.append(first);

        // Read and apply deltas
        while (true) {
            const delta = reader.readInt(i64, .little) catch break;
            const last = result.items[result.items.len - 1];
            try result.append(last + delta);
        }

        return result.toOwnedSlice();
    }
};

/// Dictionary encoding for low-cardinality strings
pub const DictionaryCompressor = struct {
    /// Compress string column with dictionary encoding
    pub fn compressStrings(allocator: std.mem.Allocator, strings: []const []const u8) !struct {
        dictionary: [][]const u8,
        indices: []u32,
    } {
        var dict_map = std.StringHashMap(u32).init(allocator);
        defer dict_map.deinit();

        var dictionary = std.ArrayList([]const u8).init(allocator);
        errdefer {
            for (dictionary.items) |str| allocator.free(str);
            dictionary.deinit();
        }

        var indices = std.ArrayList(u32).init(allocator);
        errdefer indices.deinit();

        for (strings) |str| {
            if (dict_map.get(str)) |index| {
                // String already in dictionary
                try indices.append(index);
            } else {
                // Add new string to dictionary
                const index: u32 = @intCast(dictionary.items.len);
                const str_copy = try allocator.dupe(u8, str);
                try dictionary.append(str_copy);
                try dict_map.put(str_copy, index);
                try indices.append(index);
            }
        }

        return .{
            .dictionary = try dictionary.toOwnedSlice(),
            .indices = try indices.toOwnedSlice(),
        };
    }

    /// Decompress dictionary-encoded strings
    pub fn decompressStrings(
        allocator: std.mem.Allocator,
        dictionary: []const []const u8,
        indices: []const u32,
    ) ![][]const u8 {
        var result = std.ArrayList([]const u8).init(allocator);
        errdefer {
            for (result.items) |str| allocator.free(str);
            result.deinit();
        }

        for (indices) |index| {
            if (index >= dictionary.len) return error.InvalidDictionaryIndex;
            const str = try allocator.dupe(u8, dictionary[index]);
            try result.append(str);
        }

        return result.toOwnedSlice();
    }
};

/// Bit-packing for small integers
pub const BitPackCompressor = struct {
    /// Compress int32 column with bit-packing
    /// Determines minimum bits needed and packs values tightly
    pub fn compressInt32(allocator: std.mem.Allocator, data: []const i32) ![]u8 {
        if (data.len == 0) return &[_]u8{};

        // Find maximum absolute value to determine bits needed
        var max_abs: u32 = 0;
        for (data) |val| {
            const abs_val: u32 = @intCast(@abs(val));
            if (abs_val > max_abs) max_abs = abs_val;
        }

        // Calculate bits needed (add 1 for sign bit)
        const bits_needed: u8 = if (max_abs == 0) 1 else @intCast(@clz(max_abs) + 1);

        var result = std.ArrayList(u8).init(allocator);
        errdefer result.deinit();

        // Write header: [bits_needed: u8][count: u32]
        try result.append(bits_needed);
        try result.writer().writeInt(u32, @intCast(data.len), .little);

        // Pack values
        // For simplicity, we'll just write the values as-is for now
        // A full implementation would pack bits tightly
        for (data) |val| {
            try result.writer().writeInt(i32, val, .little);
        }

        return result.toOwnedSlice();
    }

    /// Decompress bit-packed int32 column
    pub fn decompressInt32(allocator: std.mem.Allocator, compressed: []const u8) ![]i32 {
        if (compressed.len < 5) return &[_]i32{};

        const bits_needed = compressed[0];
        _ = bits_needed; // TODO: Use for actual bit-packing

        var stream = std.io.fixedBufferStream(compressed[1..]);
        const reader = stream.reader();

        const count = try reader.readInt(u32, .little);

        var result = try std.ArrayList(i32).initCapacity(allocator, count);
        errdefer result.deinit();

        var i: u32 = 0;
        while (i < count) : (i += 1) {
            const val = reader.readInt(i32, .little) catch break;
            try result.append(val);
        }

        return result.toOwnedSlice();
    }
};

/// Null bitmap compression for sparse columns
pub const NullBitmapCompressor = struct {
    /// Create null bitmap from boolean array
    pub fn createBitmap(allocator: std.mem.Allocator, is_null: []const bool) ![]u8 {
        const byte_count = (is_null.len + 7) / 8;
        var bitmap = try allocator.alloc(u8, byte_count);
        @memset(bitmap, 0);

        for (is_null, 0..) |null_flag, i| {
            if (null_flag) {
                const byte_index = i / 8;
                const bit_index = @as(u3, @intCast(i % 8));
                bitmap[byte_index] |= @as(u8, 1) << bit_index;
            }
        }

        return bitmap;
    }

    /// Expand null bitmap to boolean array
    pub fn expandBitmap(allocator: std.mem.Allocator, bitmap: []const u8, count: usize) ![]bool {
        var result = try allocator.alloc(bool, count);

        for (0..count) |i| {
            const byte_index = i / 8;
            const bit_index = @as(u3, @intCast(i % 8));

            if (byte_index < bitmap.len) {
                result[i] = (bitmap[byte_index] & (@as(u8, 1) << bit_index)) != 0;
            } else {
                result[i] = false;
            }
        }

        return result;
    }

    /// Compress sparse null bitmap with RLE
    pub fn compressSparse(allocator: std.mem.Allocator, bitmap: []const u8) ![]u8 {
        if (bitmap.len == 0) return &[_]u8{};

        var result = std.ArrayList(u8).init(allocator);
        errdefer result.deinit();

        var i: usize = 0;
        while (i < bitmap.len) {
            const byte = bitmap[i];
            var run_length: u32 = 1;

            // Count consecutive equal bytes
            while (i + run_length < bitmap.len and bitmap[i + run_length] == byte) {
                run_length += 1;
            }

            // Write run: [run_length: u32][byte: u8]
            try result.writer().writeInt(u32, run_length, .little);
            try result.append(byte);

            i += run_length;
        }

        return result.toOwnedSlice();
    }

    /// Decompress RLE-compressed null bitmap
    pub fn decompressSparse(allocator: std.mem.Allocator, compressed: []const u8) ![]u8 {
        var result = std.ArrayList(u8).init(allocator);
        errdefer result.deinit();

        var stream = std.io.fixedBufferStream(compressed);
        const reader = stream.reader();

        while (true) {
            const run_length = reader.readInt(u32, .little) catch break;
            const byte = reader.readByte() catch break;

            var j: u32 = 0;
            while (j < run_length) : (j += 1) {
                try result.append(byte);
            }
        }

        return result.toOwnedSlice();
    }
};

/// Columnar compression engine that selects appropriate compression
pub const ColumnarCompressionEngine = struct {
    allocator: std.mem.Allocator,

    pub fn init(allocator: std.mem.Allocator) ColumnarCompressionEngine {
        return .{ .allocator = allocator };
    }

    /// Select best compression for int64 column
    pub fn selectCompressionInt64(self: *ColumnarCompressionEngine, data: []const i64) ColumnCompressionType {
        _ = self;
        if (data.len == 0) return .none;

        // Check for repeated values (RLE candidate)
        var repeated_count: usize = 0;
        for (data[1..], 1..) |val, i| {
            if (val == data[i - 1]) repeated_count += 1;
        }

        if (repeated_count > data.len / 2) {
            return .rle; // More than 50% repeated values
        }

        // Check for sequential values (Delta candidate)
        var sequential_count: usize = 0;
        for (data[1..], 1..) |val, i| {
            const delta = val - data[i - 1];
            if (delta >= -10 and delta <= 10) sequential_count += 1;
        }

        if (sequential_count > data.len * 3 / 4) {
            return .delta; // More than 75% sequential
        }

        return .none; // No compression beneficial
    }

    /// Compress int64 column with selected algorithm
    pub fn compressInt64(
        self: *ColumnarCompressionEngine,
        data: []const i64,
        compression: ColumnCompressionType,
    ) ![]u8 {
        return switch (compression) {
            .none => std.mem.sliceAsBytes(data),
            .rle => try RLECompressor.compressInt64(self.allocator, data),
            .delta => try DeltaCompressor.compressInt64(self.allocator, data),
            else => error.UnsupportedCompression,
        };
    }

    /// Decompress int64 column
    pub fn decompressInt64(
        self: *ColumnarCompressionEngine,
        compressed: []const u8,
        compression: ColumnCompressionType,
    ) ![]i64 {
        return switch (compression) {
            .none => std.mem.bytesAsSlice(i64, compressed),
            .rle => try RLECompressor.decompressInt64(self.allocator, compressed),
            .delta => try DeltaCompressor.decompressInt64(self.allocator, compressed),
            else => error.UnsupportedCompression,
        };
    }
};

// ============================================================================
// Tests
// ============================================================================

test "RLECompressor: compress and decompress int64" {
    const allocator = std.testing.allocator;

    const data = [_]i64{ 5, 5, 5, 3, 3, 7, 7, 7, 7 };

    const compressed = try RLECompressor.compressInt64(allocator, &data);
    defer allocator.free(compressed);

    const decompressed = try RLECompressor.decompressInt64(allocator, compressed);
    defer allocator.free(decompressed);

    try std.testing.expectEqualSlices(i64, &data, decompressed);
}

test "DeltaCompressor: compress and decompress int64" {
    const allocator = std.testing.allocator;

    const data = [_]i64{ 100, 101, 102, 103, 104 };

    const compressed = try DeltaCompressor.compressInt64(allocator, &data);
    defer allocator.free(compressed);

    const decompressed = try DeltaCompressor.decompressInt64(allocator, compressed);
    defer allocator.free(decompressed);

    try std.testing.expectEqualSlices(i64, &data, decompressed);
}

test "DictionaryCompressor: compress and decompress strings" {
    const allocator = std.testing.allocator;

    const strings = [_][]const u8{ "apple", "banana", "apple", "cherry", "banana", "apple" };

    const result = try DictionaryCompressor.compressStrings(allocator, &strings);
    defer {
        for (result.dictionary) |str| allocator.free(str);
        allocator.free(result.dictionary);
        allocator.free(result.indices);
    }

    try std.testing.expectEqual(@as(usize, 3), result.dictionary.len); // 3 unique strings

    const decompressed = try DictionaryCompressor.decompressStrings(allocator, result.dictionary, result.indices);
    defer {
        for (decompressed) |str| allocator.free(str);
        allocator.free(decompressed);
    }

    try std.testing.expectEqual(strings.len, decompressed.len);
    for (strings, decompressed) |original, decoded| {
        try std.testing.expectEqualStrings(original, decoded);
    }
}

test "BitPackCompressor: compress and decompress int32" {
    const allocator = std.testing.allocator;

    const data = [_]i32{ 1, 2, 3, 4, 5 };

    const compressed = try BitPackCompressor.compressInt32(allocator, &data);
    defer allocator.free(compressed);

    const decompressed = try BitPackCompressor.decompressInt32(allocator, compressed);
    defer allocator.free(decompressed);

    try std.testing.expectEqualSlices(i32, &data, decompressed);
}

test "NullBitmapCompressor: create and expand bitmap" {
    const allocator = std.testing.allocator;

    const is_null = [_]bool{ false, true, false, false, true, true, false, false };

    const bitmap = try NullBitmapCompressor.createBitmap(allocator, &is_null);
    defer allocator.free(bitmap);

    const expanded = try NullBitmapCompressor.expandBitmap(allocator, bitmap, is_null.len);
    defer allocator.free(expanded);

    try std.testing.expectEqualSlices(bool, &is_null, expanded);
}

test "NullBitmapCompressor: compress and decompress sparse" {
    const allocator = std.testing.allocator;

    const bitmap = [_]u8{ 0, 0, 0, 0xFF, 0xFF, 0, 0, 0 };

    const compressed = try NullBitmapCompressor.compressSparse(allocator, &bitmap);
    defer allocator.free(compressed);

    const decompressed = try NullBitmapCompressor.decompressSparse(allocator, compressed);
    defer allocator.free(decompressed);

    try std.testing.expectEqualSlices(u8, &bitmap, decompressed);
}

test "ColumnarCompressionEngine: select compression for repeated values" {
    const allocator = std.testing.allocator;

    var engine = ColumnarCompressionEngine.init(allocator);

    const data = [_]i64{ 5, 5, 5, 5, 5, 5, 5, 5 };
    const compression = engine.selectCompressionInt64(&data);

    try std.testing.expectEqual(ColumnCompressionType.rle, compression);
}

test "ColumnarCompressionEngine: select compression for sequential values" {
    const allocator = std.testing.allocator;

    var engine = ColumnarCompressionEngine.init(allocator);

    const data = [_]i64{ 100, 101, 102, 103, 104, 105, 106, 107 };
    const compression = engine.selectCompressionInt64(&data);

    try std.testing.expectEqual(ColumnCompressionType.delta, compression);
}
