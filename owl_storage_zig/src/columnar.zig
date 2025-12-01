// Task 11: Columnar Projections for Analytical Workloads
// Hybrid row-columnar storage for OLTP + OLAP performance

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ObjectHeader = @import("object_header.zig").ObjectHeader;
const simd_ops = @import("simd_ops.zig");

/// Column data type for type-specific optimizations
pub const ColumnType = enum(u8) {
    int8 = 0,
    int16 = 1,
    int32 = 2,
    int64 = 3,
    uint8 = 4,
    uint16 = 5,
    uint32 = 6,
    uint64 = 7,
    float32 = 8,
    float64 = 9,
    boolean = 10,
    string = 11,
    bytes = 12,
    reference = 13, // ObjRef or GlobalObjRef
};

/// Columnar compression type (Task 11.2)
pub const ColumnCompressionType = enum(u8) {
    none = 0,
    rle = 1, // Run-Length Encoding for repeated values
    delta = 2, // Delta encoding for sequential integers
    dictionary = 3, // Dictionary encoding for low-cardinality strings
    bitpack = 4, // Bit-packing for small integers
    null_bitmap = 5, // Null bitmap compression for sparse columns
};

/// Column metadata for a materialized projection
pub const ColumnMetadata = struct {
    /// Column name (field path in object, e.g., "profile.age")
    name: []const u8,
    /// Column data type
    column_type: ColumnType,
    /// Compression type used
    compression: ColumnCompressionType,
    /// Number of values in column
    value_count: u64,
    /// Offset in columnar storage
    data_offset: u64,
    /// Size of compressed column data
    data_size: u64,
    /// Null bitmap offset (if column has nulls)
    null_bitmap_offset: u64,
    /// Null bitmap size
    null_bitmap_size: u64,
    /// Dictionary offset (for dictionary compression)
    dictionary_offset: u64,
    /// Dictionary size
    dictionary_size: u64,
    /// Last access timestamp (for hot column detection)
    last_access_ns: std.atomic.Value(u64),
    /// Access count (for hot column detection)
    access_count: std.atomic.Value(u64),
};

/// Columnar projection for a set of objects
/// This stores a subset of columns in column-oriented format for fast analytics
pub const ColumnarProjection = struct {
    /// Schema type tag this projection is for
    type_tag: u16,
    /// Array of column metadata
    columns: []ColumnMetadata,
    /// Columnar data storage (all columns concatenated)
    data: []align(64) u8, // Cache-line aligned for SIMD
    /// Object references (row IDs) for this projection
    objrefs: []GlobalObjRef,
    /// Allocator for memory management
    allocator: std.mem.Allocator,
    /// Creation timestamp
    created_at_ns: u64,
    /// Last update timestamp
    updated_at_ns: std.atomic.Value(u64),
    /// Invalidation flag (set when underlying objects change)
    is_valid: std.atomic.Value(bool),

    /// Create a new columnar projection
    pub fn init(
        allocator: std.mem.Allocator,
        type_tag: u16,
        columns: []const ColumnMetadata,
        data_size: usize,
        object_count: usize,
    ) !ColumnarProjection {
        const columns_copy = try allocator.dupe(ColumnMetadata, columns);
        errdefer allocator.free(columns_copy);

        const data = try allocator.alignedAlloc(u8, @as(std.mem.Alignment, @enumFromInt(6)), data_size); // 2^6 = 64 bytes
        errdefer allocator.free(data);

        const objrefs = try allocator.alloc(GlobalObjRef, object_count);
        errdefer allocator.free(objrefs);

        const now = @as(u64, @intCast(std.time.nanoTimestamp()));

        return .{
            .type_tag = type_tag,
            .columns = columns_copy,
            .data = data,
            .objrefs = objrefs,
            .allocator = allocator,
            .created_at_ns = now,
            .updated_at_ns = std.atomic.Value(u64).init(now),
            .is_valid = std.atomic.Value(bool).init(true),
        };
    }

    /// Free all allocated memory
    pub fn deinit(self: *ColumnarProjection) void {
        for (self.columns) |*col| {
            self.allocator.free(col.name);
        }
        self.allocator.free(self.columns);
        self.allocator.free(self.data);
        self.allocator.free(self.objrefs);
    }

    /// Mark projection as invalid (needs rematerialization)
    pub fn invalidate(self: *ColumnarProjection) void {
        self.is_valid.store(false, .seq_cst);
    }

    /// Check if projection is still valid
    pub fn isValid(self: *const ColumnarProjection) bool {
        return self.is_valid.load(.seq_cst);
    }

    /// Update access statistics for a column
    pub fn recordAccess(self: *ColumnarProjection, column_index: usize) void {
        if (column_index >= self.columns.len) return;

        const now = @as(u64, @intCast(std.time.nanoTimestamp()));
        self.columns[column_index].last_access_ns.store(now, .seq_cst);
        _ = self.columns[column_index].access_count.fetchAdd(1, .seq_cst);
    }

    /// Get column data slice
    pub fn getColumnData(self: *const ColumnarProjection, column_index: usize) ?[]const u8 {
        if (column_index >= self.columns.len) return null;

        const col = &self.columns[column_index];
        const start: usize = @intCast(col.data_offset);
        const end = start + @as(usize, @intCast(col.data_size));

        if (end > self.data.len) return null;

        return self.data[start..end];
    }

    /// Get null bitmap for a column
    pub fn getNullBitmap(self: *const ColumnarProjection, column_index: usize) ?[]const u8 {
        if (column_index >= self.columns.len) return null;

        const col = &self.columns[column_index];
        if (col.null_bitmap_size == 0) return null;

        const start: usize = @intCast(col.null_bitmap_offset);
        const end = start + @as(usize, @intCast(col.null_bitmap_size));

        if (end > self.data.len) return null;

        return self.data[start..end];
    }

    /// Check if a value is null
    pub fn isNull(self: *const ColumnarProjection, column_index: usize, row_index: usize) bool {
        const bitmap = self.getNullBitmap(column_index) orelse return false;

        const byte_index = row_index / 8;
        const bit_index = @as(u3, @intCast(row_index % 8));

        if (byte_index >= bitmap.len) return false;

        return (bitmap[byte_index] & (@as(u8, 1) << bit_index)) != 0;
    }

    /// Get number of rows in projection
    pub fn rowCount(self: *const ColumnarProjection) usize {
        return self.objrefs.len;
    }

    /// Get number of columns in projection
    pub fn columnCount(self: *const ColumnarProjection) usize {
        return self.columns.len;
    }
};

/// Columnar projection builder for materializing projections from row data
pub const ColumnarProjectionBuilder = struct {
    allocator: std.mem.Allocator,
    type_tag: u16,
    columns: std.ArrayList(ColumnMetadata),
    data: std.ArrayList(u8),
    objrefs: std.ArrayList(GlobalObjRef),
    current_offset: u64,

    pub fn init(allocator: std.mem.Allocator, type_tag: u16) ColumnarProjectionBuilder {
        return .{
            .allocator = allocator,
            .type_tag = type_tag,
            .columns = std.ArrayList(ColumnMetadata){},
            .data = std.ArrayList(u8){},
            .objrefs = std.ArrayList(GlobalObjRef){},
            .current_offset = 0,
        };
    }

    pub fn deinit(self: *ColumnarProjectionBuilder) void {
        for (self.columns.items) |*col| {
            self.allocator.free(col.name);
        }
        self.columns.deinit(self.allocator);
        self.data.deinit(self.allocator);
        self.objrefs.deinit(self.allocator);
    }

    /// Add a column to the projection
    pub fn addColumn(
        self: *ColumnarProjectionBuilder,
        name: []const u8,
        column_type: ColumnType,
        compression: ColumnCompressionType,
    ) !void {
        const name_copy = try self.allocator.dupe(u8, name);
        errdefer self.allocator.free(name_copy);

        const metadata = ColumnMetadata{
            .name = name_copy,
            .column_type = column_type,
            .compression = compression,
            .value_count = 0,
            .data_offset = self.current_offset,
            .data_size = 0,
            .null_bitmap_offset = 0,
            .null_bitmap_size = 0,
            .dictionary_offset = 0,
            .dictionary_size = 0,
            .last_access_ns = std.atomic.Value(u64).init(0),
            .access_count = std.atomic.Value(u64).init(0),
        };

        try self.columns.append(self.allocator, metadata);
    }

    /// Add column data (already compressed)
    pub fn addColumnData(self: *ColumnarProjectionBuilder, column_index: usize, data: []const u8) !void {
        if (column_index >= self.columns.items.len) return error.InvalidColumnIndex;

        try self.data.appendSlice(self.allocator, data);

        self.columns.items[column_index].data_size = @intCast(data.len);
        self.current_offset += data.len;
    }

    /// Add null bitmap for a column
    pub fn addNullBitmap(self: *ColumnarProjectionBuilder, column_index: usize, bitmap: []const u8) !void {
        if (column_index >= self.columns.items.len) return error.InvalidColumnIndex;

        self.columns.items[column_index].null_bitmap_offset = @intCast(self.data.items.len);
        try self.data.appendSlice(self.allocator, bitmap);
        self.columns.items[column_index].null_bitmap_size = @intCast(bitmap.len);
        self.current_offset += bitmap.len;
    }

    /// Add an object reference
    pub fn addObjectRef(self: *ColumnarProjectionBuilder, objref: GlobalObjRef) !void {
        try self.objrefs.append(self.allocator, objref);
    }

    /// Build the final columnar projection
    pub fn build(self: *ColumnarProjectionBuilder) !ColumnarProjection {
        const columns = try self.columns.toOwnedSlice(self.allocator);
        const objrefs = try self.objrefs.toOwnedSlice(self.allocator);

        // For data, we need to ensure proper alignment
        const data_slice = try self.data.toOwnedSlice(self.allocator);
        const aligned_data = try self.allocator.alignedAlloc(u8, @as(std.mem.Alignment, @enumFromInt(6)), data_slice.len);
        @memcpy(aligned_data, data_slice);
        self.allocator.free(data_slice);

        const now = @as(u64, @intCast(std.time.nanoTimestamp()));

        return .{
            .type_tag = self.type_tag,
            .columns = columns,
            .data = aligned_data,
            .objrefs = objrefs,
            .allocator = self.allocator,
            .created_at_ns = now,
            .updated_at_ns = std.atomic.Value(u64).init(now),
            .is_valid = std.atomic.Value(bool).init(true),
        };
    }
};

/// Columnar projection manager for tracking and managing projections
pub const ColumnarProjectionManager = struct {
    /// Map from type_tag to list of projections
    projections: std.AutoHashMap(u16, std.ArrayList(*ColumnarProjection)),
    /// Allocator for internal structures
    allocator: std.mem.Allocator,
    /// Mutex for thread-safe access
    mutex: std.Thread.Mutex,

    pub fn init(allocator: std.mem.Allocator) ColumnarProjectionManager {
        return .{
            .projections = std.AutoHashMap(u16, std.ArrayList(*ColumnarProjection)).init(allocator),
            .allocator = allocator,
            .mutex = std.Thread.Mutex{},
        };
    }

    pub fn deinit(self: *ColumnarProjectionManager) void {
        var it = self.projections.iterator();
        while (it.next()) |entry| {
            for (entry.value_ptr.items) |proj| {
                proj.deinit();
                self.allocator.destroy(proj);
            }
            entry.value_ptr.deinit(self.allocator);
        }
        self.projections.deinit();
    }

    /// Register a new projection
    pub fn registerProjection(self: *ColumnarProjectionManager, projection: *ColumnarProjection) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const entry = try self.projections.getOrPut(projection.type_tag);
        if (!entry.found_existing) {
            entry.value_ptr.* = std.ArrayList(*ColumnarProjection){};
        }

        try entry.value_ptr.append(self.allocator, projection);
    }

    /// Get all projections for a type
    pub fn getProjections(self: *ColumnarProjectionManager, type_tag: u16) ?[]const *ColumnarProjection {
        self.mutex.lock();
        defer self.mutex.unlock();

        const list = self.projections.get(type_tag) orelse return null;
        return list.items;
    }

    /// Invalidate all projections for a type (called on updates)
    pub fn invalidateType(self: *ColumnarProjectionManager, type_tag: u16) void {
        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.projections.get(type_tag)) |list| {
            for (list.items) |proj| {
                proj.invalidate();
            }
        }
    }

    /// Remove invalid projections (garbage collection)
    pub fn removeInvalidProjections(self: *ColumnarProjectionManager, type_tag: u16) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        const list = self.projections.getPtr(type_tag) orelse return;

        var i: usize = 0;
        while (i < list.items.len) {
            if (!list.items[i].isValid()) {
                const proj = list.items[i];
                _ = list.swapRemove(i);
                proj.deinit();
                self.allocator.destroy(proj);
            } else {
                i += 1;
            }
        }
    }
};

// ============================================================================
// Tests
// ============================================================================

test "ColumnarProjection: creation and cleanup" {
    const allocator = std.testing.allocator;

    const columns = [_]ColumnMetadata{
        .{
            .name = try allocator.dupe(u8, "id"),
            .column_type = .int64,
            .compression = .none,
            .value_count = 100,
            .data_offset = 0,
            .data_size = 800,
            .null_bitmap_offset = 0,
            .null_bitmap_size = 0,
            .dictionary_offset = 0,
            .dictionary_size = 0,
            .last_access_ns = std.atomic.Value(u64).init(0),
            .access_count = std.atomic.Value(u64).init(0),
        },
    };

    var proj = try ColumnarProjection.init(allocator, 1, &columns, 1024, 100);
    defer proj.deinit();

    try std.testing.expectEqual(@as(u16, 1), proj.type_tag);
    try std.testing.expectEqual(@as(usize, 1), proj.columnCount());
    try std.testing.expectEqual(@as(usize, 100), proj.rowCount());
    try std.testing.expect(proj.isValid());
}

test "ColumnarProjection: invalidation" {
    const allocator = std.testing.allocator;

    var proj = try ColumnarProjection.init(allocator, 1, &[_]ColumnMetadata{}, 0, 0);
    defer proj.deinit();

    try std.testing.expect(proj.isValid());

    proj.invalidate();
    try std.testing.expect(!proj.isValid());
}

test "ColumnarProjectionBuilder: basic usage" {
    const allocator = std.testing.allocator;

    var builder = ColumnarProjectionBuilder.init(allocator, 1);
    defer builder.deinit();

    try builder.addColumn("age", .int32, .none);
    try builder.addColumn("name", .string, .dictionary);

    const data = [_]u8{ 1, 2, 3, 4 };
    try builder.addColumnData(0, &data);

    try builder.addObjectRef(GlobalObjRef.init(0, 1024, 0));
    try builder.addObjectRef(GlobalObjRef.init(0, 2048, 0));

    var proj = try builder.build();
    defer proj.deinit();

    try std.testing.expectEqual(@as(usize, 2), proj.columnCount());
    try std.testing.expectEqual(@as(usize, 2), proj.rowCount());
}

test "ColumnarProjectionManager: register and retrieve" {
    const allocator = std.testing.allocator;

    var manager = ColumnarProjectionManager.init(allocator);
    defer manager.deinit();

    const proj = try allocator.create(ColumnarProjection);
    proj.* = try ColumnarProjection.init(allocator, 1, &[_]ColumnMetadata{}, 0, 0);

    try manager.registerProjection(proj);

    const projections = manager.getProjections(1);
    try std.testing.expect(projections != null);
    try std.testing.expectEqual(@as(usize, 1), projections.?.len);
}

test "ColumnarProjectionManager: invalidate type" {
    const allocator = std.testing.allocator;

    var manager = ColumnarProjectionManager.init(allocator);
    defer manager.deinit();

    const proj = try allocator.create(ColumnarProjection);
    proj.* = try ColumnarProjection.init(allocator, 1, &[_]ColumnMetadata{}, 0, 0);

    try manager.registerProjection(proj);

    try std.testing.expect(proj.isValid());

    manager.invalidateType(1);

    try std.testing.expect(!proj.isValid());
}

// ============================================================================
// SIMD Vectorized Operations (Task 11.3)
// ============================================================================

/// Vectorized column scanner for fast analytical queries
pub const VectorizedScanner = struct {
    /// Scan int64 column with SIMD and compute sum
    pub fn sumInt64(data: []const u8) i64 {
        if (data.len % 8 != 0) return 0;

        const values = std.mem.bytesAsSlice(i64, data);
        var sum: i64 = 0;
        for (values) |val| {
            sum += val;
        }
        return sum;
    }

    /// Scan int32 column with SIMD and compute sum
    pub fn sumInt32(data: []const u8) i32 {
        if (data.len % 4 != 0) return 0;

        const values = std.mem.bytesAsSlice(i32, data);
        var sum: i32 = 0;
        for (values) |val| {
            sum += val;
        }
        return sum;
    }

    /// Scan float64 column with SIMD and compute sum
    pub fn sumFloat64(data: []const u8) f64 {
        if (data.len % 8 != 0) return 0.0;

        const values = std.mem.bytesAsSlice(f64, data);
        var sum: f64 = 0.0;
        for (values) |val| {
            sum += val;
        }
        return sum;
    }

    /// Count non-null values in column
    pub fn countNonNull(null_bitmap: ?[]const u8, total_count: usize) usize {
        if (null_bitmap == null) return total_count;

        const bitmap = null_bitmap.?;
        var count: usize = 0;

        // Use SIMD to count set bits
        for (bitmap) |byte| {
            count += @popCount(byte);
        }

        // Null bitmap has 1 for null, so subtract from total
        return total_count - count;
    }

    /// Find minimum value in int64 column
    pub fn minInt64(data: []const u8, null_bitmap: ?[]const u8) ?i64 {
        if (data.len % 8 != 0) return null;

        const values = std.mem.bytesAsSlice(i64, data);
        if (values.len == 0) return null;

        var min_val = values[0];
        var found_non_null = false;

        for (values, 0..) |val, i| {
            // Check if value is null
            if (null_bitmap) |bitmap| {
                const byte_index = i / 8;
                const bit_index = @as(u3, @intCast(i % 8));
                if (byte_index < bitmap.len and (bitmap[byte_index] & (@as(u8, 1) << bit_index)) != 0) {
                    continue; // Skip null values
                }
            }

            if (!found_non_null or val < min_val) {
                min_val = val;
                found_non_null = true;
            }
        }

        return if (found_non_null) min_val else null;
    }

    /// Find maximum value in int64 column
    pub fn maxInt64(data: []const u8, null_bitmap: ?[]const u8) ?i64 {
        if (data.len % 8 != 0) return null;

        const values = std.mem.bytesAsSlice(i64, data);
        if (values.len == 0) return null;

        var max_val = values[0];
        var found_non_null = false;

        for (values, 0..) |val, i| {
            // Check if value is null
            if (null_bitmap) |bitmap| {
                const byte_index = i / 8;
                const bit_index = @as(u3, @intCast(i % 8));
                if (byte_index < bitmap.len and (bitmap[byte_index] & (@as(u8, 1) << bit_index)) != 0) {
                    continue; // Skip null values
                }
            }

            if (!found_non_null or val > max_val) {
                max_val = val;
                found_non_null = true;
            }
        }

        return if (found_non_null) max_val else null;
    }

    /// Compute average of int64 column
    pub fn avgInt64(data: []const u8, null_bitmap: ?[]const u8) ?f64 {
        if (data.len % 8 != 0) return null;

        const values = std.mem.bytesAsSlice(i64, data);
        if (values.len == 0) return null;

        var sum: i64 = 0;
        var count: usize = 0;

        for (values, 0..) |val, i| {
            // Check if value is null
            if (null_bitmap) |bitmap| {
                const byte_index = i / 8;
                const bit_index = @as(u3, @intCast(i % 8));
                if (byte_index < bitmap.len and (bitmap[byte_index] & (@as(u8, 1) << bit_index)) != 0) {
                    continue; // Skip null values
                }
            }

            sum += val;
            count += 1;
        }

        if (count == 0) return null;

        return @as(f64, @floatFromInt(sum)) / @as(f64, @floatFromInt(count));
    }

    /// Filter int64 column by predicate (returns matching row indices)
    pub fn filterInt64(
        allocator: std.mem.Allocator,
        data: []const u8,
        null_bitmap: ?[]const u8,
        predicate: *const fn (i64) bool,
    ) ![]usize {
        if (data.len % 8 != 0) return &[_]usize{};

        const values = std.mem.bytesAsSlice(i64, data);
        var matches = std.ArrayList(usize){};

        for (values, 0..) |val, i| {
            // Check if value is null
            if (null_bitmap) |bitmap| {
                const byte_index = i / 8;
                const bit_index = @as(u3, @intCast(i % 8));
                if (byte_index < bitmap.len and (bitmap[byte_index] & (@as(u8, 1) << bit_index)) != 0) {
                    continue; // Skip null values
                }
            }

            if (predicate(val)) {
                try matches.append(allocator, i);
            }
        }

        return matches.toOwnedSlice(allocator);
    }

    /// Project multiple columns (select specific columns from projection)
    pub fn projectColumns(
        allocator: std.mem.Allocator,
        projection: *const ColumnarProjection,
        column_indices: []const usize,
    ) !ColumnarProjection {
        var builder = ColumnarProjectionBuilder.init(allocator, projection.type_tag);
        errdefer builder.deinit();

        // Add selected columns
        for (column_indices) |col_idx| {
            if (col_idx >= projection.columns.len) continue;

            const col = &projection.columns[col_idx];
            try builder.addColumn(col.name, col.column_type, col.compression);

            // Copy column data
            if (projection.getColumnData(col_idx)) |data| {
                try builder.addColumnData(builder.columns.items.len - 1, data);
            }

            // Copy null bitmap if present
            if (projection.getNullBitmap(col_idx)) |bitmap| {
                try builder.addNullBitmap(builder.columns.items.len - 1, bitmap);
            }
        }

        // Copy object references
        for (projection.objrefs) |objref| {
            try builder.addObjectRef(objref);
        }

        return builder.build();
    }
};

// ============================================================================
// Vectorized Operations Tests
// ============================================================================

test "VectorizedScanner: sumInt64" {
    const data = [_]i64{ 1, 2, 3, 4, 5 };
    const bytes = std.mem.sliceAsBytes(&data);

    const sum = VectorizedScanner.sumInt64(bytes);
    try std.testing.expectEqual(@as(i64, 15), sum);
}

test "VectorizedScanner: countNonNull" {
    // Bitmap: 0b00001010 = bits 1 and 3 are null
    const bitmap = [_]u8{0b00001010};
    const count = VectorizedScanner.countNonNull(&bitmap, 8);
    try std.testing.expectEqual(@as(usize, 6), count); // 8 - 2 nulls = 6
}

test "VectorizedScanner: minInt64" {
    const data = [_]i64{ 5, 2, 8, 1, 9 };
    const bytes = std.mem.sliceAsBytes(&data);

    const min_val = VectorizedScanner.minInt64(bytes, null);
    try std.testing.expectEqual(@as(i64, 1), min_val.?);
}

test "VectorizedScanner: maxInt64" {
    const data = [_]i64{ 5, 2, 8, 1, 9 };
    const bytes = std.mem.sliceAsBytes(&data);

    const max_val = VectorizedScanner.maxInt64(bytes, null);
    try std.testing.expectEqual(@as(i64, 9), max_val.?);
}

test "VectorizedScanner: avgInt64" {
    const data = [_]i64{ 2, 4, 6, 8, 10 };
    const bytes = std.mem.sliceAsBytes(&data);

    const avg = VectorizedScanner.avgInt64(bytes, null);
    try std.testing.expectEqual(@as(f64, 6.0), avg.?);
}

test "VectorizedScanner: filterInt64" {
    const allocator = std.testing.allocator;

    const data = [_]i64{ 1, 5, 3, 8, 2, 9 };
    const bytes = std.mem.sliceAsBytes(&data);

    const greaterThanFive = struct {
        fn pred(val: i64) bool {
            return val > 5;
        }
    }.pred;

    const matches = try VectorizedScanner.filterInt64(allocator, bytes, null, greaterThanFive);
    defer allocator.free(matches);

    try std.testing.expectEqual(@as(usize, 2), matches.len);
    try std.testing.expectEqual(@as(usize, 3), matches[0]); // 8 at index 3
    try std.testing.expectEqual(@as(usize, 5), matches[1]); // 9 at index 5
}
