// Task 2.1: C API - Core Operations
// C API exports for Rust integration

const std = @import("std");
const Arena = @import("arena.zig").Arena;
const BTree = @import("btree.zig").BTree;
const BTreeConfig = @import("btree.zig").BTreeConfig;
const HnswIndex = @import("hnsw_index.zig").HnswIndex;
const ObjRef = @import("objref.zig").ObjRef;
const Object = @import("object.zig").Object;
const ObjectHeader = @import("object_header.zig").ObjectHeader;

// C-compatible error codes - Enhanced for cross-layer error handling
pub const OWL_OK: c_int = 0;
pub const OWL_ERR_OUT_OF_MEMORY: c_int = -1;
pub const OWL_ERR_INVALID_ARENA: c_int = -2;
pub const OWL_ERR_INVALID_REF: c_int = -3;
pub const OWL_ERR_DATA_TOO_SMALL: c_int = -4;
pub const OWL_ERR_OUT_OF_BOUNDS: c_int = -5;
pub const OWL_ERR_OBJECT_DELETED: c_int = -6;
pub const OWL_ERR_INVALID_TRANSACTION: c_int = -7;
pub const OWL_ERR_TRANSACTION_NOT_FOUND: c_int = -8;
pub const OWL_ERR_TRANSACTION_NOT_ACTIVE: c_int = -9;
pub const OWL_ERR_TRANSACTION_TIMED_OUT: c_int = -10;
pub const OWL_ERR_PREPARE_FAILURE: c_int = -11;
pub const OWL_ERR_COMMIT_FAILURE: c_int = -12;
pub const OWL_ERR_ABORT_FAILURE: c_int = -13;
pub const OWL_ERR_INVALID_OPERATION: c_int = -14;
pub const OWL_ERR_INVALID_SHARD: c_int = -15;
pub const OWL_ERR_SHARD_NOT_FOUND: c_int = -16;
pub const OWL_ERR_WAL_ERROR: c_int = -17;
pub const OWL_ERR_STORAGE_ERROR: c_int = -18;
pub const OWL_ERR_NETWORK_ERROR: c_int = -19;
pub const OWL_ERR_SERIALIZATION_ERROR: c_int = -20;
pub const OWL_ERR_DESERIALIZATION_ERROR: c_int = -21;
pub const OWL_ERR_CHECKSUM_MISMATCH: c_int = -22;
pub const OWL_ERR_CORRUPTED_DATA: c_int = -23;
pub const OWL_ERR_INSUFFICIENT_SPACE: c_int = -24;
pub const OWL_ERR_LOCK_CONFLICT: c_int = -25;
pub const OWL_ERR_DEADLOCK_DETECTED: c_int = -26;
pub const OWL_ERR_INVALID_SIZE: c_int = -27;
pub const OWL_ERR_UNKNOWN: c_int = -999;

// Global allocator for C API (uses libc allocator)
var gpa = std.heap.c_allocator;

const SUPERBLOCK_MAGIC: u32 = 0x3142534f; // "OSB1"
const SUPERBLOCK_VERSION: u16 = 1;
const SUPERBLOCK_COPIES: usize = 4;
const SUPERBLOCK_RECORD_SIZE: usize = 128;
const WAL_MAGIC: u32 = 0x4c41574f; // "OWAL"
const WAL_VERSION: u8 = 1;
const WAL_FRAME_META_SIZE: usize = 12;
const WAL_ENTRY_SIZE_MAX: usize = 64 * 1024 * 1024;

const SuperblockRecord = struct {
    copy: u16,
    sequence: u64,
    arena_size: u64,
    wal_size: u64,
    document_count: u64,
    schema_count: u32,
    parent_crc32: u32,
    record_crc32: u32,
};

fn crc32(bytes: []const u8) u32 {
    var hasher = std.hash.Crc32.init();
    hasher.update(bytes);
    return hasher.final();
}

fn encodeSuperblockRecord(record: SuperblockRecord) [SUPERBLOCK_RECORD_SIZE]u8 {
    var out = std.mem.zeroes([SUPERBLOCK_RECORD_SIZE]u8);
    std.mem.writeInt(u32, out[0..4], SUPERBLOCK_MAGIC, .little);
    std.mem.writeInt(u16, out[4..6], SUPERBLOCK_VERSION, .little);
    std.mem.writeInt(u16, out[6..8], record.copy, .little);
    std.mem.writeInt(u64, out[8..16], record.sequence, .little);
    std.mem.writeInt(u64, out[16..24], record.arena_size, .little);
    std.mem.writeInt(u64, out[24..32], record.wal_size, .little);
    std.mem.writeInt(u64, out[32..40], record.document_count, .little);
    std.mem.writeInt(u32, out[40..44], record.schema_count, .little);
    std.mem.writeInt(u32, out[44..48], 0, .little); // reserved
    std.mem.writeInt(u32, out[48..52], record.parent_crc32, .little);
    const record_crc = crc32(out[0..52]);
    std.mem.writeInt(u32, out[52..56], record_crc, .little);
    return out;
}

fn decodeSuperblockRecord(buf: []const u8) ?SuperblockRecord {
    if (buf.len < SUPERBLOCK_RECORD_SIZE) return null;
    const magic = std.mem.readInt(u32, buf[0..4], .little);
    if (magic != SUPERBLOCK_MAGIC) return null;
    const version = std.mem.readInt(u16, buf[4..6], .little);
    if (version != SUPERBLOCK_VERSION) return null;
    const copy = std.mem.readInt(u16, buf[6..8], .little);
    if (copy >= SUPERBLOCK_COPIES) return null;
    const sequence = std.mem.readInt(u64, buf[8..16], .little);
    const arena_size = std.mem.readInt(u64, buf[16..24], .little);
    const wal_size = std.mem.readInt(u64, buf[24..32], .little);
    const document_count = std.mem.readInt(u64, buf[32..40], .little);
    const schema_count = std.mem.readInt(u32, buf[40..44], .little);
    const parent_crc32 = std.mem.readInt(u32, buf[48..52], .little);
    const record_crc32 = std.mem.readInt(u32, buf[52..56], .little);
    const computed = crc32(buf[0..52]);
    if (computed != record_crc32) return null;
    return .{
        .copy = copy,
        .sequence = sequence,
        .arena_size = arena_size,
        .wal_size = wal_size,
        .document_count = document_count,
        .schema_count = schema_count,
        .parent_crc32 = parent_crc32,
        .record_crc32 = record_crc32,
    };
}

fn openFileRWCreate(path: []const u8) !std.fs.File {
    const cwd = std.fs.cwd();
    return cwd.openFile(path, .{ .mode = .read_write }) catch |err| switch (err) {
        error.FileNotFound => try cwd.createFile(path, .{ .read = true, .truncate = false }),
        else => err,
    };
}

/// Recover best superblock record.
/// Returns:
/// - 1 if a valid record was found (outputs populated)
/// - 0 if no usable superblock exists
/// - negative error code on failure (strict failures return OWL_ERR_CORRUPTED_DATA)
export fn owl_superblock_recover(
    path: [*:0]const u8,
    strict: bool,
    out_sequence: ?*u64,
    out_arena_size: ?*u64,
    out_wal_size: ?*u64,
    out_document_count: ?*u64,
    out_schema_count: ?*u32,
    out_record_crc32: ?*u32,
) c_int {
    const path_slice = std.mem.span(path);
    const file = std.fs.cwd().openFile(path_slice, .{ .mode = .read_only }) catch |err| switch (err) {
        error.FileNotFound => return 0,
        else => return OWL_ERR_STORAGE_ERROR,
    };
    defer file.close();

    const file_size = file.getEndPos() catch return OWL_ERR_STORAGE_ERROR;
    if (file_size < SUPERBLOCK_RECORD_SIZE) return 0;

    const readable_copies = @min(SUPERBLOCK_COPIES, @as(usize, @intCast(file_size / SUPERBLOCK_RECORD_SIZE)));
    var records: [SUPERBLOCK_COPIES]SuperblockRecord = undefined;
    var record_count: usize = 0;

    var copy: usize = 0;
    while (copy < readable_copies) : (copy += 1) {
        var buf: [SUPERBLOCK_RECORD_SIZE]u8 = undefined;
        _ = file.preadAll(&buf, copy * SUPERBLOCK_RECORD_SIZE) catch break;
        if (decodeSuperblockRecord(&buf)) |rec| {
            records[record_count] = rec;
            record_count += 1;
        }
    }

    if (record_count == 0) return 0;

    var quorum_found = false;
    var winner: SuperblockRecord = records[0];
    var winner_votes: usize = 0;
    var i: usize = 0;
    while (i < record_count) : (i += 1) {
        var count: usize = 0;
        var j: usize = 0;
        while (j < record_count) : (j += 1) {
            if (records[j].sequence == records[i].sequence and records[j].record_crc32 == records[i].record_crc32) {
                count += 1;
            }
        }
        if (count >= 2 and (!quorum_found or records[i].sequence > winner.sequence or (records[i].sequence == winner.sequence and count > winner_votes))) {
            quorum_found = true;
            winner = records[i];
            winner_votes = count;
        }
    }

    if (quorum_found) {
        if (strict and winner.sequence > 1) {
            var has_parent = false;
            i = 0;
            while (i < record_count) : (i += 1) {
                if (records[i].sequence + 1 == winner.sequence and records[i].record_crc32 == winner.parent_crc32) {
                    has_parent = true;
                    break;
                }
            }
            if (!has_parent) return OWL_ERR_CORRUPTED_DATA;
        }
    } else {
        if (strict) return OWL_ERR_CORRUPTED_DATA;
        i = 1;
        while (i < record_count) : (i += 1) {
            if (records[i].sequence > winner.sequence) winner = records[i];
        }
    }

    if (out_sequence) |ptr| ptr.* = winner.sequence;
    if (out_arena_size) |ptr| ptr.* = winner.arena_size;
    if (out_wal_size) |ptr| ptr.* = winner.wal_size;
    if (out_document_count) |ptr| ptr.* = winner.document_count;
    if (out_schema_count) |ptr| ptr.* = winner.schema_count;
    if (out_record_crc32) |ptr| ptr.* = winner.record_crc32;
    return 1;
}

/// Persist superblock record copyset.
/// `initial_full=true` writes all 4 copies; otherwise writes two rotating copies.
export fn owl_superblock_persist(
    path: [*:0]const u8,
    sequence: u64,
    parent_crc32: u32,
    arena_size: u64,
    wal_size: u64,
    document_count: u64,
    schema_count: u32,
    initial_full: bool,
    out_record_crc32: ?*u32,
) c_int {
    const path_slice = std.mem.span(path);
    var file = openFileRWCreate(path_slice) catch return OWL_ERR_STORAGE_ERROR;
    defer file.close();

    const min_size = SUPERBLOCK_RECORD_SIZE * SUPERBLOCK_COPIES;
    file.setEndPos(min_size) catch return OWL_ERR_STORAGE_ERROR;

    var last_crc = parent_crc32;
    if (initial_full or sequence == 1) {
        var copy: usize = 0;
        while (copy < SUPERBLOCK_COPIES) : (copy += 1) {
            const record = SuperblockRecord{
                .copy = @as(u16, @intCast(copy)),
                .sequence = sequence,
                .arena_size = arena_size,
                .wal_size = wal_size,
                .document_count = document_count,
                .schema_count = schema_count,
                .parent_crc32 = parent_crc32,
                .record_crc32 = 0,
            };
            const encoded = encodeSuperblockRecord(record);
            file.pwriteAll(&encoded, copy * SUPERBLOCK_RECORD_SIZE) catch return OWL_ERR_STORAGE_ERROR;
            last_crc = std.mem.readInt(u32, encoded[52..56], .little);
        }
    } else {
        const a = @as(usize, @intCast(sequence % SUPERBLOCK_COPIES));
        const b = (a + 1) % SUPERBLOCK_COPIES;
        const copies = [_]usize{ a, b };
        for (copies) |copy| {
            const record = SuperblockRecord{
                .copy = @as(u16, @intCast(copy)),
                .sequence = sequence,
                .arena_size = arena_size,
                .wal_size = wal_size,
                .document_count = document_count,
                .schema_count = schema_count,
                .parent_crc32 = parent_crc32,
                .record_crc32 = 0,
            };
            const encoded = encodeSuperblockRecord(record);
            file.pwriteAll(&encoded, copy * SUPERBLOCK_RECORD_SIZE) catch return OWL_ERR_STORAGE_ERROR;
            last_crc = std.mem.readInt(u32, encoded[52..56], .little);
        }
    }

    file.sync() catch return OWL_ERR_STORAGE_ERROR;
    if (out_record_crc32) |ptr| ptr.* = last_crc;
    return OWL_OK;
}

/// Append a framed WAL record (TigerBeetle-style framing + CRC).
export fn owl_wal_append_framed(
    path: [*:0]const u8,
    payload: ?[*]const u8,
    payload_len: usize,
) c_int {
    if (payload_len == 0 or payload_len > WAL_ENTRY_SIZE_MAX) return OWL_ERR_INVALID_SIZE;
    const payload_slice = (payload orelse return OWL_ERR_INVALID_REF)[0..payload_len];

    const path_slice = std.mem.span(path);
    var file = openFileRWCreate(path_slice) catch return OWL_ERR_STORAGE_ERROR;
    defer file.close();

    const end_pos = file.getEndPos() catch return OWL_ERR_STORAGE_ERROR;
    file.seekTo(end_pos) catch return OWL_ERR_STORAGE_ERROR;

    var frame_meta: [WAL_FRAME_META_SIZE]u8 = std.mem.zeroes([WAL_FRAME_META_SIZE]u8);
    frame_meta[0] = WAL_VERSION;
    frame_meta[1] = 0; // flags
    std.mem.writeInt(u16, frame_meta[2..4], 0, .little); // reserved
    std.mem.writeInt(u32, frame_meta[4..8], @as(u32, @intCast(payload_len)), .little);
    std.mem.writeInt(u32, frame_meta[8..12], crc32(payload_slice), .little);

    var magic_bytes: [4]u8 = undefined;
    std.mem.writeInt(u32, &magic_bytes, WAL_MAGIC, .little);
    file.writeAll(&magic_bytes) catch return OWL_ERR_STORAGE_ERROR;
    file.writeAll(&frame_meta) catch return OWL_ERR_STORAGE_ERROR;
    file.writeAll(payload_slice) catch return OWL_ERR_STORAGE_ERROR;
    file.sync() catch return OWL_ERR_STORAGE_ERROR;
    return OWL_OK;
}

/// Create a new arena
/// Returns null on failure
export fn owl_arena_create(path: [*:0]const u8, size: usize) ?*Arena {
    const path_slice = std.mem.span(path);
    const arena = gpa.create(Arena) catch return null;
    arena.* = Arena.init(gpa, path_slice, size, 0) catch { // Default to shard 0
        gpa.destroy(arena);
        return null;
    };
    return arena;
}

/// Destroy an arena
export fn owl_arena_destroy(arena: ?*Arena) void {
    if (arena) |a| {
        a.deinit();
        gpa.destroy(a);
    }
}

/// Allocate an object
export fn owl_arena_alloc(
    arena: ?*Arena,
    size: u32,
    out_ref: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = a.alloc(size) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            error.SizeTooBig => OWL_ERR_INVALID_SIZE,
            error.InvalidSize => OWL_ERR_INVALID_SIZE,
        };
    };
    if (out_ref) |ptr| {
        ptr.* = ref.getOffset();
    }
    return OWL_OK;
}

/// Allocate an object near another object (for co-location)
export fn owl_arena_alloc_near(
    arena: ?*Arena,
    size: u32,
    near: u64,
    out_ref: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const near_ref: ?ObjRef = if (near == 0) null else ObjRef.init(near);
    const ref = a.allocNear(size, near_ref) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            error.SizeTooBig => OWL_ERR_INVALID_SIZE,
            error.InvalidSize => OWL_ERR_INVALID_SIZE,
        };
    };
    if (out_ref) |ptr| {
        ptr.* = ref.getOffset();
    }
    return OWL_OK;
}

/// Write data to an object
export fn owl_arena_write(
    arena: ?*Arena,
    obj_ref: u64,
    data: ?[*]const u8,
    data_len: usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = ObjRef.init(obj_ref);
    const data_slice = (data orelse return OWL_ERR_INVALID_REF)[0..data_len];
    a.write(ref, data_slice) catch |err| {
        return switch (err) {
            error.InvalidReference => OWL_ERR_INVALID_REF,
            error.DataTooSmall => OWL_ERR_DATA_TOO_SMALL,
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
        };
    };
    return OWL_OK;
}

/// Read an object from the arena
/// Note: Caller must free the returned object data
export fn owl_arena_read(
    arena: ?*Arena,
    obj_ref: u64,
    buffer: ?[*]u8,
    buffer_len: usize,
    out_bytes_read: ?*usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = ObjRef.init(obj_ref);
    const buf = (buffer orelse return OWL_ERR_INVALID_REF)[0..buffer_len];

    // Read object
    var obj = a.read(ref, gpa) catch |err| {
        return switch (err) {
            error.InvalidReference => OWL_ERR_INVALID_REF,
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
            error.ObjectDeleted => OWL_ERR_OBJECT_DELETED,
            else => OWL_ERR_INVALID_ARENA,
        };
    };
    defer obj.deinit();

    // Serialize to buffer
    const bytes = obj.toBytes(gpa) catch return OWL_ERR_INVALID_ARENA;
    defer gpa.free(bytes);

    const copy_len = @min(buf.len, bytes.len);
    @memcpy(buf[0..copy_len], bytes[0..copy_len]);

    if (out_bytes_read) |ptr| {
        ptr.* = copy_len;
    }

    return OWL_OK;
}

/// Delete an object
export fn owl_arena_delete(
    arena: ?*Arena,
    obj_ref: u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = ObjRef.init(obj_ref);
    a.delete(ref) catch |err| {
        return switch (err) {
            error.InvalidReference => OWL_ERR_INVALID_REF,
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
            else => OWL_ERR_INVALID_ARENA,
        };
    };
    return OWL_OK;
}

/// Get arena statistics
export fn owl_arena_stats(
    arena: ?*Arena,
    out_total: ?*usize,
    out_used: ?*usize,
    out_free: ?*usize,
    out_object_count: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const arena_stats = a.stats();
    if (out_total) |ptr| ptr.* = arena_stats.total_size;
    if (out_used) |ptr| ptr.* = arena_stats.used_size;
    if (out_free) |ptr| ptr.* = arena_stats.free_size;
    if (out_object_count) |ptr| ptr.* = arena_stats.object_count;
    return OWL_OK;
}

/// Get co-location metrics
export fn owl_arena_colocation_metrics(
    arena: ?*Arena,
    out_total_attempts: ?*usize,
    out_successful: ?*usize,
    out_failed: ?*usize,
    out_success_rate: ?*f64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const metrics = a.colocationMetrics();
    if (out_total_attempts) |ptr| ptr.* = metrics.total_attempts.load(.seq_cst);
    if (out_successful) |ptr| ptr.* = metrics.successful_colocations.load(.seq_cst);
    if (out_failed) |ptr| ptr.* = metrics.failed_colocations.load(.seq_cst);
    if (out_success_rate) |ptr| ptr.* = metrics.successRate();
    return OWL_OK;
}

/// Create a B-tree index handle.
export fn owl_btree_create(order: u32, shard_id: u32) ?*BTree {
    if (order < 2) return null;
    const config = BTreeConfig{ .order = @as(usize, @intCast(order)) };
    return BTree.init(gpa, config, shard_id) catch null;
}

/// Destroy a B-tree index handle.
export fn owl_btree_destroy(btree: ?*BTree) void {
    if (btree) |tree| {
        tree.deinit();
    }
}

/// Insert key/value into B-tree.
export fn owl_btree_insert(
    btree: ?*BTree,
    key: u64,
    value_offset: u64,
) c_int {
    const tree = btree orelse return OWL_ERR_INVALID_REF;
    tree.insert(key, ObjRef.init(value_offset)) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            else => OWL_ERR_UNKNOWN,
        };
    };
    return OWL_OK;
}

/// Search key in B-tree.
export fn owl_btree_search(
    btree: ?*BTree,
    key: u64,
    out_value_offset: ?*u64,
    out_found: ?*bool,
) c_int {
    const tree = btree orelse return OWL_ERR_INVALID_REF;
    const found = tree.search(key);
    if (out_found) |ptr| ptr.* = (found != null);
    if (found) |objref| {
        if (out_value_offset) |ptr| ptr.* = objref.getOffset();
    }
    return OWL_OK;
}

/// Delete key from B-tree.
export fn owl_btree_delete(
    btree: ?*BTree,
    key: u64,
    out_deleted: ?*bool,
) c_int {
    const tree = btree orelse return OWL_ERR_INVALID_REF;
    const deleted = tree.delete(key) catch return OWL_ERR_UNKNOWN;
    if (out_deleted) |ptr| ptr.* = deleted;
    return OWL_OK;
}

/// Range query for key interval [start_key, end_key].
/// If `out_value_offsets` is null, only `out_count` is written.
/// If `out_value_offsets` is non-null and `capacity` is too small,
/// returns `OWL_ERR_INSUFFICIENT_SPACE` and writes required count.
export fn owl_btree_range_query(
    btree: ?*BTree,
    start_key: u64,
    end_key: u64,
    out_value_offsets: ?[*]u64,
    capacity: usize,
    out_count: ?*usize,
) c_int {
    const tree = btree orelse return OWL_ERR_INVALID_REF;
    const results = tree.rangeQuery(start_key, end_key, gpa) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
        };
    };
    defer gpa.free(results);

    if (out_count) |ptr| ptr.* = results.len;

    if (out_value_offsets == null) return OWL_OK;
    if (capacity < results.len) return OWL_ERR_INSUFFICIENT_SPACE;

    const out = out_value_offsets.?;
    for (results, 0..) |objref, i| {
        out[i] = objref.getOffset();
    }
    return OWL_OK;
}

/// Exact top-k vector search over a dense row-major f32 matrix.
///
/// Metric encoding:
/// - 0: cosine similarity (higher is better)
/// - 1: negative L2 distance (higher is better)
/// - 2: dot product (higher is better)
export fn owl_vector_topk_f32(
    vectors_ptr: ?[*]const f32,
    vector_count: usize,
    dimension: usize,
    query_ptr: ?[*]const f32,
    metric: u32,
    k: usize,
    out_indices: ?[*]u32,
    out_scores: ?[*]f32,
    out_count: ?*usize,
) c_int {
    if (vector_count == 0 or dimension == 0 or k == 0) {
        if (out_count) |ptr| ptr.* = 0;
        return OWL_OK;
    }

    const vectors = vectors_ptr orelse return OWL_ERR_INVALID_REF;
    const query = query_ptr orelse return OWL_ERR_INVALID_REF;
    const indices_out = out_indices orelse return OWL_ERR_INVALID_REF;
    const scores_out = out_scores orelse return OWL_ERR_INVALID_REF;
    if (out_count == null) return OWL_ERR_INVALID_REF;

    const top_k = @min(k, vector_count);
    const count_out = out_count.?;
    count_out.* = top_k;

    const total_values = vector_count * dimension;
    const matrix = vectors[0..total_values];
    const query_vec = query[0..dimension];

    const ScoredIndex = struct {
        idx: u32,
        score: f32,
    };

    var scored = gpa.alloc(ScoredIndex, vector_count) catch return OWL_ERR_OUT_OF_MEMORY;
    defer gpa.free(scored);

    for (0..vector_count) |row| {
        const start = row * dimension;
        const vec = matrix[start .. start + dimension];

        var dot: f64 = 0;
        var norm_vec: f64 = 0;
        var norm_query: f64 = 0;
        var sq_dist: f64 = 0;

        for (0..dimension) |i| {
            const a = @as(f64, vec[i]);
            const b = @as(f64, query_vec[i]);
            dot += a * b;
            norm_vec += a * a;
            norm_query += b * b;
            const d = a - b;
            sq_dist += d * d;
        }

        const score: f32 = switch (metric) {
            0 => blk: {
                if (norm_vec <= 0 or norm_query <= 0) break :blk 0.0;
                break :blk @floatCast(dot / (std.math.sqrt(norm_vec) * std.math.sqrt(norm_query)));
            },
            1 => @floatCast(-std.math.sqrt(sq_dist)),
            2 => @floatCast(dot),
            else => return OWL_ERR_INVALID_SIZE,
        };

        scored[row] = .{
            .idx = @intCast(row),
            .score = score,
        };
    }

    std.mem.sort(ScoredIndex, scored, {}, struct {
        fn lessThan(_: void, a: ScoredIndex, b: ScoredIndex) bool {
            return a.score > b.score;
        }
    }.lessThan);

    for (0..top_k) |i| {
        indices_out[i] = scored[i].idx;
        scores_out[i] = scored[i].score;
    }

    return OWL_OK;
}

/// Create a new HNSW index handle.
export fn owl_hnsw_index_create(
    dimension: u32,
    m: u32,
    ef_construction: u32,
) ?*HnswIndex {
    if (dimension == 0) return null;
    const index = gpa.create(HnswIndex) catch return null;
    index.* = HnswIndex.init(gpa, dimension, m, ef_construction);
    return index;
}

/// Destroy an HNSW index handle.
export fn owl_hnsw_index_destroy(index: ?*HnswIndex) void {
    if (index) |idx| {
        idx.deinit();
        gpa.destroy(idx);
    }
}

/// Add a single vector to the HNSW index.
export fn owl_hnsw_index_add_vector(
    index: ?*HnswIndex,
    id: u64,
    vector_ptr: ?[*]const f32,
    vector_len: usize,
) c_int {
    const idx = index orelse return OWL_ERR_INVALID_REF;
    const vector = (vector_ptr orelse return OWL_ERR_INVALID_REF)[0..vector_len];
    idx.addVector(id, vector) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            error.InvalidDimension => OWL_ERR_INVALID_SIZE,
        };
    };
    return OWL_OK;
}

/// Search the HNSW index.
export fn owl_hnsw_index_search(
    index: ?*HnswIndex,
    query_ptr: ?[*]const f32,
    query_len: usize,
    k: usize,
    ef: usize,
    out_ids: ?[*]u64,
    out_scores: ?[*]f32,
    out_count: ?*usize,
) c_int {
    const idx = index orelse return OWL_ERR_INVALID_REF;
    if (k == 0) {
        if (out_count) |ptr| ptr.* = 0;
        return OWL_OK;
    }

    const query = (query_ptr orelse return OWL_ERR_INVALID_REF)[0..query_len];
    const ids = out_ids orelse return OWL_ERR_INVALID_REF;
    const scores = out_scores orelse return OWL_ERR_INVALID_REF;
    const count_out = out_count orelse return OWL_ERR_INVALID_REF;

    const hits = idx.search(query, k, ef, gpa) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            error.InvalidDimension => OWL_ERR_INVALID_SIZE,
        };
    };
    defer gpa.free(hits);

    count_out.* = hits.len;
    for (hits, 0..) |hit, i| {
        ids[i] = hit.id;
        scores[i] = hit.score;
    }
    return OWL_OK;
}

/// Persist HNSW index to disk.
export fn owl_hnsw_index_save(
    index: ?*HnswIndex,
    path: [*:0]const u8,
) c_int {
    const idx = index orelse return OWL_ERR_INVALID_REF;
    const path_slice = std.mem.span(path);
    idx.saveToFile(path_slice) catch |err| {
        return switch (err) {
            error.AccessDenied, error.FileNotFound, error.PathAlreadyExists => OWL_ERR_STORAGE_ERROR,
            else => OWL_ERR_UNKNOWN,
        };
    };
    return OWL_OK;
}

/// Load HNSW index from disk.
export fn owl_hnsw_index_load(path: [*:0]const u8) ?*HnswIndex {
    const path_slice = std.mem.span(path);
    const idx = gpa.create(HnswIndex) catch return null;
    idx.* = HnswIndex.loadFromFile(gpa, path_slice) catch {
        gpa.destroy(idx);
        return null;
    };
    return idx;
}

/// Get the base pointer for swizzling (advanced usage)
export fn owl_arena_base_ptr(arena: ?*Arena) ?[*]u8 {
    const a = arena orelse return null;
    return a.basePtr();
}

/// Write raw bytes at a specific offset (low-level API)
export fn owl_arena_write_at(
    arena: ?*Arena,
    offset: u64,
    data: ?[*]const u8,
    data_len: usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const data_slice = (data orelse return OWL_ERR_INVALID_REF)[0..data_len];
    a.writeAt(offset, data_slice) catch |err| {
        return switch (err) {
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
        };
    };
    return OWL_OK;
}

/// Read raw bytes from a specific offset (low-level API)
export fn owl_arena_read_at(
    arena: ?*Arena,
    offset: u64,
    buffer: ?[*]u8,
    buffer_len: usize,
    out_bytes_read: ?*usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const buf = (buffer orelse return OWL_ERR_INVALID_REF)[0..buffer_len];

    const data = a.readAt(offset, buffer_len) catch |err| {
        return switch (err) {
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
        };
    };

    const copy_len = @min(buf.len, data.len);
    @memcpy(buf[0..copy_len], data[0..copy_len]);

    if (out_bytes_read) |ptr| {
        ptr.* = copy_len;
    }

    return OWL_OK;
}

// ============================================================================
// Tests
// ============================================================================

test "C API: arena lifecycle" {
    const path = "/tmp/test_c_api_arena.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);
}

test "C API: allocation" {
    const path = "/tmp/test_c_api_alloc.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    var ref: u64 = 0;
    const result = owl_arena_alloc(arena, 128, &ref);
    try std.testing.expectEqual(OWL_OK, result);
    try std.testing.expect(ref != 0);
}

test "C API: write and read" {
    const path = "/tmp/test_c_api_rw.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    // Create an object
    const id = [_]u8{1} ** 16;
    const fixed = [_]u8{ 1, 2, 3, 4 };
    var obj = try Object.init(gpa, 1, id, &fixed, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(gpa);
    defer gpa.free(data);

    // Allocate and write
    var ref: u64 = 0;
    _ = owl_arena_alloc(arena, @intCast(data.len), &ref);
    const write_result = owl_arena_write(arena, ref, data.ptr, data.len);
    try std.testing.expectEqual(OWL_OK, write_result);

    // Read back
    var buffer: [1024]u8 = undefined;
    var bytes_read: usize = 0;
    const read_result = owl_arena_read(arena, ref, &buffer, buffer.len, &bytes_read);
    try std.testing.expectEqual(OWL_OK, read_result);
    try std.testing.expect(bytes_read > 0);
}

test "C API: delete" {
    const path = "/tmp/test_c_api_delete.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    // Create and write object
    const id = [_]u8{1} ** 16;
    var obj = try Object.init(gpa, 1, id, &[_]u8{ 1, 2, 3 }, &[_]u8{}, &[_]ObjRef{});
    defer obj.deinit();

    const data = try obj.toBytes(gpa);
    defer gpa.free(data);

    var ref: u64 = 0;
    _ = owl_arena_alloc(arena, @intCast(data.len), &ref);
    _ = owl_arena_write(arena, ref, data.ptr, data.len);

    // Delete
    const delete_result = owl_arena_delete(arena, ref);
    try std.testing.expectEqual(OWL_OK, delete_result);

    // Try to read - should fail
    var buffer: [1024]u8 = undefined;
    var bytes_read: usize = 0;
    const read_result = owl_arena_read(arena, ref, &buffer, buffer.len, &bytes_read);
    try std.testing.expectEqual(OWL_ERR_OBJECT_DELETED, read_result);
}

test "C API: stats" {
    const path = "/tmp/test_c_api_stats.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    var total: usize = 0;
    var used: usize = 0;
    var free: usize = 0;
    var count: u64 = 0;

    const result = owl_arena_stats(arena, &total, &used, &free, &count);
    try std.testing.expectEqual(OWL_OK, result);
    try std.testing.expectEqual(@as(usize, 1024 * 1024), total);
    try std.testing.expectEqual(@as(u64, 0), count);
}

test "C API: co-location" {
    const path = "/tmp/test_c_api_coloc.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    try std.testing.expect(arena != null);
    defer owl_arena_destroy(arena);

    var ref1: u64 = 0;
    _ = owl_arena_alloc(arena, 100, &ref1);

    var ref2: u64 = 0;
    const result = owl_arena_alloc_near(arena, 100, ref1, &ref2);
    try std.testing.expectEqual(OWL_OK, result);

    // Check metrics
    var total: usize = 0;
    var successful: usize = 0;
    var failed: usize = 0;
    var rate: f64 = 0;

    _ = owl_arena_colocation_metrics(arena, &total, &successful, &failed, &rate);
    try std.testing.expect(total > 0);
}

test "C API: error handling" {
    // Null arena
    var ref: u64 = 0;
    const result = owl_arena_alloc(null, 100, &ref);
    try std.testing.expectEqual(OWL_ERR_INVALID_ARENA, result);

    // Invalid reference
    const path = "/tmp/test_c_api_err.dat";
    const arena = owl_arena_create(path, 1024 * 1024);
    defer owl_arena_destroy(arena);

    const write_result = owl_arena_write(arena, 0, null, 0);
    try std.testing.expectEqual(OWL_ERR_INVALID_REF, write_result);
}

// ============================================================================
// Deterministic Arena C API (Task 2.2.1)
// ============================================================================

const DeterministicArena = @import("deterministic_arena.zig").DeterministicArena;
const AllocationStrategy = @import("deterministic_arena.zig").AllocationStrategy;
const GCConfig = @import("deterministic_arena.zig").GCConfig;

/// Create a new deterministic arena
/// Returns null on failure
export fn owl_deterministic_arena_create(
    path: [*:0]const u8,
    size: usize,
    seed: u64,
    strategy: u32, // AllocationStrategy as u32
    gc_enabled: bool,
    gc_trigger_interval: u32,
    gc_collection_percentage: u8,
) ?*DeterministicArena {
    const path_slice = std.mem.span(path);

    const alloc_strategy: AllocationStrategy = switch (strategy) {
        0 => .bump,
        1 => .random_placement,
        2 => .fragmentation_sim,
        3 => .deterministic_colocation,
        else => .bump,
    };

    const gc_config = GCConfig{
        .enabled = gc_enabled,
        .trigger_interval = gc_trigger_interval,
        .collection_percentage = gc_collection_percentage,
        .deterministic_selection = true,
    };

    const arena = gpa.create(DeterministicArena) catch return null;
    arena.* = DeterministicArena.init(gpa, path_slice, size, seed, alloc_strategy, gc_config) catch {
        gpa.destroy(arena);
        return null;
    };
    return arena;
}

/// Destroy a deterministic arena
export fn owl_deterministic_arena_destroy(arena: ?*DeterministicArena) void {
    if (arena) |a| {
        a.deinit();
        gpa.destroy(a);
    }
}

/// Allocate an object in deterministic arena
export fn owl_deterministic_arena_alloc(
    arena: ?*DeterministicArena,
    size: u32,
    out_ref: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const ref = a.alloc(size) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
        };
    };

    if (out_ref) |out| {
        out.* = ref.getOffset();
    }
    return OWL_OK;
}

/// Allocate near another object in deterministic arena
export fn owl_deterministic_arena_alloc_near(
    arena: ?*DeterministicArena,
    size: u32,
    near_offset: u64,
    out_ref: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const near_ref = if (near_offset == 0) null else ObjRef.init(near_offset);

    const ref = a.allocNear(size, near_ref) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
        };
    };

    if (out_ref) |out| {
        out.* = ref.getOffset();
    }
    return OWL_OK;
}

/// Reset deterministic arena to initial state
export fn owl_deterministic_arena_reset(arena: ?*DeterministicArena) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    a.reset();
    return OWL_OK;
}

/// Set allocation strategy
export fn owl_deterministic_arena_set_strategy(
    arena: ?*DeterministicArena,
    strategy: u32,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;

    const alloc_strategy: AllocationStrategy = switch (strategy) {
        0 => .bump,
        1 => .random_placement,
        2 => .fragmentation_sim,
        3 => .deterministic_colocation,
        else => return OWL_ERR_INVALID_ARENA,
    };

    a.setStrategy(alloc_strategy);
    return OWL_OK;
}

/// Enable or disable allocation logging
export fn owl_deterministic_arena_set_logging(
    arena: ?*DeterministicArena,
    enabled: bool,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    a.setLogging(enabled);
    return OWL_OK;
}

/// Get current RNG state
export fn owl_deterministic_arena_get_rng_state(
    arena: ?*DeterministicArena,
    out_state: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    if (out_state) |out| {
        out.* = a.getRNGState();
    }
    return OWL_OK;
}

/// Write data to deterministic arena
export fn owl_deterministic_arena_write_at(
    arena: ?*DeterministicArena,
    offset: u64,
    data: [*]const u8,
    len: usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const data_slice = data[0..len];

    a.writeAt(offset, data_slice) catch |err| {
        return switch (err) {
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
        };
    };

    return OWL_OK;
}

/// Read data from deterministic arena
export fn owl_deterministic_arena_read_at(
    arena: ?*DeterministicArena,
    offset: u64,
    buffer: [*]u8,
    buffer_len: usize,
    out_bytes_read: ?*usize,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const buffer_slice = buffer[0..buffer_len];

    const data = a.readAt(offset, buffer_len) catch |err| {
        return switch (err) {
            error.OutOfBounds => OWL_ERR_OUT_OF_BOUNDS,
        };
    };

    const bytes_to_copy = @min(data.len, buffer_len);
    @memcpy(buffer_slice[0..bytes_to_copy], data[0..bytes_to_copy]);

    if (out_bytes_read) |out| {
        out.* = bytes_to_copy;
    }

    return OWL_OK;
}

/// Get deterministic arena statistics
export fn owl_deterministic_arena_stats(
    arena: ?*DeterministicArena,
    out_total: ?*usize,
    out_used: ?*usize,
    out_free: ?*usize,
    out_object_count: ?*u64,
    out_allocation_sequence: ?*u64,
    out_rng_state: ?*u64,
) c_int {
    const a = arena orelse return OWL_ERR_INVALID_ARENA;
    const stats = a.stats();

    if (out_total) |out| out.* = stats.total_size;
    if (out_used) |out| out.* = stats.used_size;
    if (out_free) |out| out.* = stats.free_size;
    if (out_object_count) |out| out.* = stats.object_count;
    if (out_allocation_sequence) |out| out.* = stats.allocation_sequence;
    if (out_rng_state) |out| out.* = stats.rng_state;

    return OWL_OK;
}

// ============================================================================
// Cross-Layer Observability C API (Task 8.15)
// ============================================================================

const Observability = @import("observability.zig");
const ObservabilityManager = Observability.ObservabilityManager;
const ArenaOperation = Observability.ArenaOperation;
const BTreeOperation = Observability.BTreeOperation;
const WALOperation = Observability.WALOperation;
const TransactionOperation = Observability.TransactionOperation;
const LogLevel = Observability.LogLevel;

/// Initialize Zig observability system
export fn zig_observability_init() bool {
    Observability.initGlobalObservability(gpa) catch return false;
    return true;
}

/// Cleanup Zig observability system
export fn zig_observability_cleanup() void {
    Observability.deinitGlobalObservability();
}

/// Collect arena metrics from Zig
export fn zig_collect_arena_metrics(
    shard_id: c_uint,
    total_allocations: *c_ulong,
    successful_allocations: *c_ulong,
    failed_allocations: *c_ulong,
    total_bytes_allocated: *c_ulong,
    total_bytes_freed: *c_ulong,
    current_usage_bytes: *c_ulong,
    peak_usage_bytes: *c_ulong,
    fragmentation_ratio: *f64,
    avg_allocation_size: *f64,
    allocation_latency_ns: *c_ulong,
    colocation_success_rate: *f64,
) void {
    if (Observability.getGlobalObservability()) |manager| {
        const metrics = manager.getMetrics(shard_id);
        const arena_metrics = &metrics.arena_metrics;

        total_allocations.* = arena_metrics.total_allocations.load(.seq_cst);
        successful_allocations.* = arena_metrics.successful_allocations.load(.seq_cst);
        failed_allocations.* = arena_metrics.failed_allocations.load(.seq_cst);
        total_bytes_allocated.* = arena_metrics.total_bytes_allocated.load(.seq_cst);
        total_bytes_freed.* = arena_metrics.total_bytes_freed.load(.seq_cst);
        current_usage_bytes.* = arena_metrics.current_usage_bytes.load(.seq_cst);
        peak_usage_bytes.* = arena_metrics.peak_usage_bytes.load(.seq_cst);
        fragmentation_ratio.* = arena_metrics.getFragmentationRatio();
        avg_allocation_size.* = arena_metrics.getAverageAllocationSize();
        allocation_latency_ns.* = arena_metrics.getAverageAllocationLatency();
        colocation_success_rate.* = arena_metrics.getColocationSuccessRate();
    } else {
        // Return zeros if observability is not initialized
        total_allocations.* = 0;
        successful_allocations.* = 0;
        failed_allocations.* = 0;
        total_bytes_allocated.* = 0;
        total_bytes_freed.* = 0;
        current_usage_bytes.* = 0;
        peak_usage_bytes.* = 0;
        fragmentation_ratio.* = 0.0;
        avg_allocation_size.* = 0.0;
        allocation_latency_ns.* = 0;
        colocation_success_rate.* = 0.0;
    }
}

/// Collect B-tree metrics from Zig
export fn zig_collect_btree_metrics(
    shard_id: c_uint,
    total_insertions: *c_ulong,
    total_deletions: *c_ulong,
    total_lookups: *c_ulong,
    successful_lookups: *c_ulong,
    node_splits: *c_ulong,
    node_merges: *c_ulong,
    tree_height: *c_uint,
    total_nodes: *c_ulong,
    avg_lookup_latency_ns: *c_ulong,
    avg_insertion_latency_ns: *c_ulong,
    cache_hit_rate: *f64,
) void {
    if (Observability.getGlobalObservability()) |manager| {
        const metrics = manager.getMetrics(shard_id);
        const btree_metrics = &metrics.btree_metrics;

        total_insertions.* = btree_metrics.total_insertions.load(.seq_cst);
        total_deletions.* = btree_metrics.total_deletions.load(.seq_cst);
        total_lookups.* = btree_metrics.total_lookups.load(.seq_cst);
        successful_lookups.* = btree_metrics.successful_lookups.load(.seq_cst);
        node_splits.* = btree_metrics.node_splits.load(.seq_cst);
        node_merges.* = btree_metrics.node_merges.load(.seq_cst);
        tree_height.* = btree_metrics.tree_height.load(.seq_cst);
        total_nodes.* = btree_metrics.total_nodes.load(.seq_cst);
        avg_lookup_latency_ns.* = btree_metrics.getAverageLookupLatency();
        avg_insertion_latency_ns.* = btree_metrics.getAverageInsertionLatency();
        cache_hit_rate.* = btree_metrics.getCacheHitRate();
    } else {
        // Return zeros if observability is not initialized
        total_insertions.* = 0;
        total_deletions.* = 0;
        total_lookups.* = 0;
        successful_lookups.* = 0;
        node_splits.* = 0;
        node_merges.* = 0;
        tree_height.* = 0;
        total_nodes.* = 0;
        avg_lookup_latency_ns.* = 0;
        avg_insertion_latency_ns.* = 0;
        cache_hit_rate.* = 0.0;
    }
}

/// Collect WAL metrics from Zig
export fn zig_collect_wal_metrics(
    shard_id: c_uint,
    total_writes: *c_ulong,
    total_bytes_written: *c_ulong,
    total_syncs: *c_ulong,
    avg_write_latency_ns: *c_ulong,
    avg_sync_latency_ns: *c_ulong,
    current_wal_size_bytes: *c_ulong,
    checkpoint_count: *c_ulong,
    last_checkpoint_timestamp: *c_ulong,
    has_checkpoint: *bool,
    write_throughput_bytes_per_sec: *f64,
) void {
    if (Observability.getGlobalObservability()) |manager| {
        const metrics = manager.getMetrics(shard_id);
        const wal_metrics = &metrics.wal_metrics;

        total_writes.* = wal_metrics.total_writes.load(.seq_cst);
        total_bytes_written.* = wal_metrics.total_bytes_written.load(.seq_cst);
        total_syncs.* = wal_metrics.total_syncs.load(.seq_cst);
        avg_write_latency_ns.* = wal_metrics.getAverageWriteLatency();
        avg_sync_latency_ns.* = wal_metrics.getAverageSyncLatency();
        current_wal_size_bytes.* = wal_metrics.current_wal_size_bytes.load(.seq_cst);
        checkpoint_count.* = wal_metrics.checkpoint_count.load(.seq_cst);

        const last_checkpoint = wal_metrics.last_checkpoint_timestamp_ns.load(.seq_cst);
        if (last_checkpoint > 0) {
            last_checkpoint_timestamp.* = last_checkpoint;
            has_checkpoint.* = true;
        } else {
            last_checkpoint_timestamp.* = 0;
            has_checkpoint.* = false;
        }

        // Calculate throughput over last minute (approximation)
        write_throughput_bytes_per_sec.* = wal_metrics.getWriteThroughput(60_000_000_000); // 60 seconds in nanoseconds
    } else {
        // Return zeros if observability is not initialized
        total_writes.* = 0;
        total_bytes_written.* = 0;
        total_syncs.* = 0;
        avg_write_latency_ns.* = 0;
        avg_sync_latency_ns.* = 0;
        current_wal_size_bytes.* = 0;
        checkpoint_count.* = 0;
        last_checkpoint_timestamp.* = 0;
        has_checkpoint.* = false;
        write_throughput_bytes_per_sec.* = 0.0;
    }
}

/// Collect transaction metrics from Zig
export fn zig_collect_transaction_metrics(
    shard_id: c_uint,
    active_transactions: *c_ulong,
    committed_transactions: *c_ulong,
    aborted_transactions: *c_ulong,
    avg_transaction_duration_ms: *f64,
    lock_conflicts: *c_ulong,
    deadlocks_detected: *c_ulong,
    two_phase_commits: *c_ulong,
    distributed_transactions: *c_ulong,
) void {
    if (Observability.getGlobalObservability()) |manager| {
        const metrics = manager.getMetrics(shard_id);
        const txn_metrics = &metrics.transaction_metrics;

        active_transactions.* = txn_metrics.active_transactions.load(.seq_cst);
        committed_transactions.* = txn_metrics.committed_transactions.load(.seq_cst);
        aborted_transactions.* = txn_metrics.aborted_transactions.load(.seq_cst);
        avg_transaction_duration_ms.* = txn_metrics.getAverageTransactionDuration();
        lock_conflicts.* = txn_metrics.lock_conflicts.load(.seq_cst);
        deadlocks_detected.* = txn_metrics.deadlocks_detected.load(.seq_cst);
        two_phase_commits.* = txn_metrics.two_phase_commits.load(.seq_cst);
        distributed_transactions.* = txn_metrics.distributed_transactions.load(.seq_cst);
    } else {
        // Return zeros if observability is not initialized
        active_transactions.* = 0;
        committed_transactions.* = 0;
        aborted_transactions.* = 0;
        avg_transaction_duration_ms.* = 0.0;
        lock_conflicts.* = 0;
        deadlocks_detected.* = 0;
        two_phase_commits.* = 0;
        distributed_transactions.* = 0;
    }
}

/// Collect memory metrics from Zig
export fn zig_collect_memory_metrics(
    shard_id: c_uint,
    total_allocated_bytes: *c_ulong,
    peak_allocated_bytes: *c_ulong,
    current_rss_bytes: *c_ulong,
    gc_cycles: *c_ulong,
    gc_time_ms: *c_ulong,
    objects_collected: *c_ulong,
    memory_pressure_events: *c_ulong,
) void {
    if (Observability.getGlobalObservability()) |manager| {
        const metrics = manager.getMetrics(shard_id);
        const memory_metrics = &metrics.memory_metrics;

        total_allocated_bytes.* = memory_metrics.total_allocated_bytes.load(.seq_cst);
        peak_allocated_bytes.* = memory_metrics.peak_allocated_bytes.load(.seq_cst);
        current_rss_bytes.* = memory_metrics.current_rss_bytes.load(.seq_cst);
        gc_cycles.* = memory_metrics.gc_cycles.load(.seq_cst);
        gc_time_ms.* = memory_metrics.gc_time_ms.load(.seq_cst);
        objects_collected.* = memory_metrics.objects_collected.load(.seq_cst);
        memory_pressure_events.* = memory_metrics.memory_pressure_events.load(.seq_cst);
    } else {
        // Return zeros if observability is not initialized
        total_allocated_bytes.* = 0;
        peak_allocated_bytes.* = 0;
        current_rss_bytes.* = 0;
        gc_cycles.* = 0;
        gc_time_ms.* = 0;
        objects_collected.* = 0;
        memory_pressure_events.* = 0;
    }
}

/// Collect I/O metrics from Zig
export fn zig_collect_io_metrics(
    shard_id: c_uint,
    total_reads: *c_ulong,
    total_writes: *c_ulong,
    total_bytes_read: *c_ulong,
    total_bytes_written: *c_ulong,
    avg_read_latency_ns: *c_ulong,
    avg_write_latency_ns: *c_ulong,
    io_errors: *c_ulong,
    cache_hits: *c_ulong,
    cache_misses: *c_ulong,
) void {
    if (Observability.getGlobalObservability()) |manager| {
        const metrics = manager.getMetrics(shard_id);
        const io_metrics = &metrics.io_metrics;

        total_reads.* = io_metrics.total_reads.load(.seq_cst);
        total_writes.* = io_metrics.total_writes.load(.seq_cst);
        total_bytes_read.* = io_metrics.total_bytes_read.load(.seq_cst);
        total_bytes_written.* = io_metrics.total_bytes_written.load(.seq_cst);
        avg_read_latency_ns.* = io_metrics.getAverageReadLatency();
        avg_write_latency_ns.* = io_metrics.getAverageWriteLatency();
        io_errors.* = io_metrics.io_errors.load(.seq_cst);
        cache_hits.* = io_metrics.cache_hits.load(.seq_cst);
        cache_misses.* = io_metrics.cache_misses.load(.seq_cst);
    } else {
        // Return zeros if observability is not initialized
        total_reads.* = 0;
        total_writes.* = 0;
        total_bytes_read.* = 0;
        total_bytes_written.* = 0;
        avg_read_latency_ns.* = 0;
        avg_write_latency_ns.* = 0;
        io_errors.* = 0;
        cache_hits.* = 0;
        cache_misses.* = 0;
    }
}

/// Start trace in Zig
export fn zig_start_trace(trace_id: [*:0]const u8, operation_name: [*:0]const u8) bool {
    if (Observability.getGlobalObservability()) |manager| {
        const trace_id_slice = std.mem.span(trace_id);
        _ = operation_name; // Mark as used

        // For now, just record that we started a trace
        // Full trace implementation would require more complex state management
        manager.log(.Info, "trace", "Started trace", 0, trace_id_slice, null);
        return true;
    }
    return false;
}

/// Start span in Zig
export fn zig_start_span(
    trace_id: [*:0]const u8,
    span_id: [*:0]const u8,
    operation_name: [*:0]const u8,
    shard_id: c_uint,
) bool {
    if (Observability.getGlobalObservability()) |manager| {
        const trace_id_slice = std.mem.span(trace_id);
        const span_id_slice = std.mem.span(span_id);
        const operation_slice = std.mem.span(operation_name);

        manager.startSpan(trace_id_slice, span_id_slice, operation_slice, shard_id) catch return false;
        return true;
    }
    return false;
}

/// Finish span in Zig
export fn zig_finish_span(trace_id: [*:0]const u8, span_id: [*:0]const u8) bool {
    if (Observability.getGlobalObservability()) |manager| {
        const trace_id_slice = std.mem.span(trace_id);
        const span_id_slice = std.mem.span(span_id);

        manager.finishSpan(trace_id_slice, span_id_slice) catch return false;
        return true;
    }
    return false;
}

/// Finish trace in Zig
export fn zig_finish_trace(trace_id: [*:0]const u8) bool {
    if (Observability.getGlobalObservability()) |manager| {
        const trace_id_slice = std.mem.span(trace_id);

        // Log that we finished the trace
        manager.log(.Info, "trace", "Finished trace", 0, trace_id_slice, null);
        return true;
    }
    return false;
}

/// Get span count for trace
export fn zig_get_span_count(trace_id: [*:0]const u8) c_uint {
    if (Observability.getGlobalObservability()) |manager| {
        const trace_id_slice = std.mem.span(trace_id);

        if (manager.getTraceInfo(trace_id_slice)) |trace_info| {
            return trace_info.getSpanCount();
        }
    }
    return 0;
}

/// Get span information
export fn zig_get_span_info(
    trace_id: [*:0]const u8,
    span_index: c_uint,
    span_id_buffer: [*]u8,
    span_id_buffer_size: usize,
    operation_buffer: [*]u8,
    operation_buffer_size: usize,
    start_time_ns: *c_ulong,
    end_time_ns: *c_ulong,
    has_end_time: *bool,
    shard_id: *c_uint,
    tag_count: *c_uint,
) bool {
    if (Observability.getGlobalObservability()) |manager| {
        const trace_id_slice = std.mem.span(trace_id);

        if (manager.getTraceInfo(trace_id_slice)) |trace_info| {
            if (trace_info.getSpan(span_index)) |span| {
                // Copy span ID
                const span_id_len = @min(span.span_id.len, span_id_buffer_size - 1);
                @memcpy(span_id_buffer[0..span_id_len], span.span_id[0..span_id_len]);
                span_id_buffer[span_id_len] = 0; // Null terminate

                // Copy operation name
                const operation_len = @min(span.operation_name.len, operation_buffer_size - 1);
                @memcpy(operation_buffer[0..operation_len], span.operation_name[0..operation_len]);
                operation_buffer[operation_len] = 0; // Null terminate

                // Set timing information
                start_time_ns.* = span.start_time_ns;
                if (span.end_time_ns) |end_time| {
                    end_time_ns.* = end_time;
                    has_end_time.* = true;
                } else {
                    end_time_ns.* = 0;
                    has_end_time.* = false;
                }

                shard_id.* = span.shard_id;
                tag_count.* = @intCast(span.tags.count());

                return true;
            }
        }
    }
    return false;
}

/// Get span tag
export fn zig_get_span_tag(
    trace_id: [*:0]const u8,
    span_id: [*:0]const u8,
    tag_index: c_uint,
    key_buffer: [*]u8,
    key_buffer_size: usize,
    value_buffer: [*]u8,
    value_buffer_size: usize,
) bool {
    if (Observability.getGlobalObservability()) |manager| {
        const trace_id_slice = std.mem.span(trace_id);
        const span_id_slice = std.mem.span(span_id);

        if (manager.getTraceInfo(trace_id_slice)) |trace_info| {
            // Find the span
            for (trace_info.spans.items) |*span| {
                if (std.mem.eql(u8, span.span_id, span_id_slice)) {
                    // Find the tag by index
                    var current_index: u32 = 0;
                    var iterator = span.tags.iterator();
                    while (iterator.next()) |entry| {
                        if (current_index == tag_index) {
                            // Copy key
                            const key_len = @min(entry.key_ptr.len, key_buffer_size - 1);
                            @memcpy(key_buffer[0..key_len], entry.key_ptr.*[0..key_len]);
                            key_buffer[key_len] = 0; // Null terminate

                            // Copy value
                            const value_len = @min(entry.value_ptr.len, value_buffer_size - 1);
                            @memcpy(value_buffer[0..value_len], entry.value_ptr.*[0..value_len]);
                            value_buffer[value_len] = 0; // Null terminate

                            return true;
                        }
                        current_index += 1;
                    }
                    break;
                }
            }
        }
    }
    return false;
}

/// Record arena operation from C
export fn zig_record_arena_operation(
    shard_id: c_uint,
    operation: c_uint, // ArenaOperation as u32
    bytes: c_ulong,
    latency_ns: c_ulong,
    success: bool,
) void {
    const arena_operation: ArenaOperation = switch (operation) {
        0 => .Allocate,
        1 => .Deallocate,
        2 => .AllocateNear,
        3 => .Write,
        4 => .Read,
        5 => .GarbageCollect,
        else => .Allocate,
    };

    Observability.recordArenaOperation(shard_id, arena_operation, bytes, latency_ns, success);
}

/// Record B-tree operation from C
export fn zig_record_btree_operation(
    shard_id: c_uint,
    operation: c_uint, // BTreeOperation as u32
    latency_ns: c_ulong,
    success: bool,
) void {
    const btree_operation: BTreeOperation = switch (operation) {
        0 => .Insert,
        1 => .Delete,
        2 => .Lookup,
        3 => .Split,
        4 => .Merge,
        5 => .Rebalance,
        else => .Lookup,
    };

    Observability.recordBTreeOperation(shard_id, btree_operation, latency_ns, success);
}

/// Record WAL operation from C
export fn zig_record_wal_operation(
    shard_id: c_uint,
    operation: c_uint, // WALOperation as u32
    bytes: c_ulong,
    latency_ns: c_ulong,
    success: bool,
) void {
    const wal_operation: WALOperation = switch (operation) {
        0 => .Write,
        1 => .Sync,
        2 => .Checkpoint,
        3 => .Replay,
        4 => .Truncate,
        else => .Write,
    };

    Observability.recordWALOperation(shard_id, wal_operation, bytes, latency_ns, success);
}

/// Record transaction operation from C
export fn zig_record_transaction_operation(
    shard_id: c_uint,
    operation: c_uint, // TransactionOperation as u32
    duration_ms: f64,
    success: bool,
) void {
    const txn_operation: TransactionOperation = switch (operation) {
        0 => .Begin,
        1 => .Commit,
        2 => .Abort,
        3 => .Prepare,
        4 => .TwoPhaseCommit,
        else => .Begin,
    };

    Observability.recordTransactionOperation(shard_id, txn_operation, duration_ms, success);
}

/// Log message from C
export fn zig_log_message(
    level: c_uint, // LogLevel as u32
    component: [*:0]const u8,
    message: [*:0]const u8,
    shard_id: c_uint,
    trace_id: ?[*:0]const u8,
    span_id: ?[*:0]const u8,
) void {
    const log_level: LogLevel = switch (level) {
        0 => .Trace,
        1 => .Debug,
        2 => .Info,
        3 => .Warn,
        4 => .Error,
        5 => .Fatal,
        else => .Info,
    };

    const component_slice = std.mem.span(component);
    const message_slice = std.mem.span(message);
    const trace_id_slice = if (trace_id) |tid| std.mem.span(tid) else null;
    const span_id_slice = if (span_id) |sid| std.mem.span(sid) else null;

    Observability.logMessage(log_level, component_slice, message_slice, shard_id, trace_id_slice, span_id_slice);
}

/// Record timing from C
export fn zig_record_timing(operation: [*:0]const u8, duration_ns: c_ulong) void {
    const operation_slice = std.mem.span(operation);
    Observability.recordTiming(operation_slice, duration_ns);
}

// ============================================================================
// Observability C API Tests
// ============================================================================

test "C API: observability initialization" {
    const result = zig_observability_init();
    try std.testing.expect(result);
    defer zig_observability_cleanup();
}

test "C API: metrics collection" {
    _ = zig_observability_init();
    defer zig_observability_cleanup();

    // Record some operations
    zig_record_arena_operation(0, 0, 1024, 1000, true); // Allocate
    zig_record_btree_operation(0, 2, 500, true); // Lookup
    zig_record_wal_operation(0, 0, 2048, 1500, true); // Write
    zig_record_transaction_operation(0, 1, 100.0, true); // Commit

    // Collect metrics
    var arena_total: c_ulong = 0;
    var arena_successful: c_ulong = 0;
    var arena_failed: c_ulong = 0;
    var arena_bytes_allocated: c_ulong = 0;
    var arena_bytes_freed: c_ulong = 0;
    var arena_current_usage: c_ulong = 0;
    var arena_peak_usage: c_ulong = 0;
    var arena_fragmentation: f64 = 0.0;
    var arena_avg_size: f64 = 0.0;
    var arena_latency: c_ulong = 0;
    var arena_colocation_rate: f64 = 0.0;

    zig_collect_arena_metrics(
        0,
        &arena_total,
        &arena_successful,
        &arena_failed,
        &arena_bytes_allocated,
        &arena_bytes_freed,
        &arena_current_usage,
        &arena_peak_usage,
        &arena_fragmentation,
        &arena_avg_size,
        &arena_latency,
        &arena_colocation_rate,
    );

    try std.testing.expectEqual(@as(c_ulong, 1), arena_total);
    try std.testing.expectEqual(@as(c_ulong, 1), arena_successful);
    try std.testing.expectEqual(@as(c_ulong, 1024), arena_bytes_allocated);
}

test "C API: tracing" {
    _ = zig_observability_init();
    defer zig_observability_cleanup();

    const trace_id = "test_trace_123";
    const span_id = "test_span_456";
    const operation = "test_operation";

    // Start trace and span
    const trace_result = zig_start_trace(trace_id, operation);
    try std.testing.expect(trace_result);

    const span_result = zig_start_span(trace_id, span_id, operation, 0);
    try std.testing.expect(span_result);

    // Finish span and trace
    const finish_span_result = zig_finish_span(trace_id, span_id);
    try std.testing.expect(finish_span_result);

    const finish_trace_result = zig_finish_trace(trace_id);
    try std.testing.expect(finish_trace_result);
}

test "C API: logging" {
    _ = zig_observability_init();
    defer zig_observability_cleanup();

    const component = "test_component";
    const message = "Test log message";
    const trace_id = "trace_123";
    const span_id = "span_456";

    // Log a message
    zig_log_message(2, component, message, 0, trace_id, span_id); // Info level

    // Record timing
    zig_record_timing("test_operation", 1000000); // 1ms in nanoseconds
}

// ============================================================================
// Backup and Recovery C API (Task 8.16)
// ============================================================================

const BackupRecovery = @import("backup_recovery.zig").BackupRecovery;
const BackupConfig = @import("backup_recovery.zig").BackupConfig;
const BackupType = @import("backup_recovery.zig").BackupType;
const CompressionType = @import("backup_recovery.zig").CompressionType;
const StorageBackend = @import("backup_recovery.zig").StorageBackend;
const RecoveryOptions = @import("backup_recovery.zig").RecoveryOptions;
const WAL = @import("wal.zig").WAL;

// Backup and Recovery error codes
pub const OWL_ERR_BACKUP_NOT_FOUND: c_int = -30;
pub const OWL_ERR_BACKUP_CORRUPTED: c_int = -31;
pub const OWL_ERR_BACKUP_CHAIN_BROKEN: c_int = -32;
pub const OWL_ERR_INCREMENTAL_DISABLED: c_int = -33;
pub const OWL_ERR_RECOVERY_FAILED: c_int = -34;

/// Create a backup and recovery manager
export fn owl_backup_recovery_create(
    arena: ?*Arena,
    wal: ?*WAL,
    backup_directory: [*:0]const u8,
    compression_type: c_int,
    storage_backend: c_int,
) ?*BackupRecovery {
    const a = arena orelse return null;
    const w = wal orelse return null;
    const backup_dir = std.mem.span(backup_directory);

    const compression = switch (compression_type) {
        0 => CompressionType.none,
        1 => CompressionType.lz4,
        2 => CompressionType.zstd,
        else => CompressionType.lz4,
    };

    const backend = switch (storage_backend) {
        0 => StorageBackend.local,
        1 => StorageBackend.s3,
        2 => StorageBackend.network,
        else => StorageBackend.local,
    };

    const config = BackupConfig{
        .backup_directory = backup_dir,
        .compression_type = compression,
        .storage_backend = backend,
    };

    const backup_recovery = gpa.create(BackupRecovery) catch return null;
    backup_recovery.* = BackupRecovery.init(gpa, a, w, config) catch {
        gpa.destroy(backup_recovery);
        return null;
    };

    return backup_recovery;
}

/// Destroy a backup and recovery manager
export fn owl_backup_recovery_destroy(backup_recovery: ?*BackupRecovery) void {
    if (backup_recovery) |br| {
        br.deinit();
        gpa.destroy(br);
    }
}

/// Create a snapshot backup
export fn owl_backup_create_snapshot(
    backup_recovery: ?*BackupRecovery,
    description: [*:0]const u8,
    out_backup_id: ?*[16]u8,
) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;
    const desc = std.mem.span(description);

    const metadata = br.createSnapshot(desc) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            error.AccessDenied => OWL_ERR_STORAGE_ERROR,
            else => OWL_ERR_UNKNOWN,
        };
    };

    if (out_backup_id) |ptr| {
        ptr.* = metadata.backup_id;
    }

    return OWL_OK;
}

/// Create a full backup
export fn owl_backup_create_full(
    backup_recovery: ?*BackupRecovery,
    description: [*:0]const u8,
    out_backup_id: ?*[16]u8,
) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;
    const desc = std.mem.span(description);

    const metadata = br.createFullBackup(desc) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            error.AccessDenied => OWL_ERR_STORAGE_ERROR,
            else => OWL_ERR_UNKNOWN,
        };
    };

    if (out_backup_id) |ptr| {
        ptr.* = metadata.backup_id;
    }

    return OWL_OK;
}

/// Create an incremental backup
export fn owl_backup_create_incremental(
    backup_recovery: ?*BackupRecovery,
    description: [*:0]const u8,
    out_backup_id: ?*[16]u8,
) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;
    const desc = std.mem.span(description);

    const metadata = br.createIncrementalBackup(desc) catch |err| {
        return switch (err) {
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            error.IncrementalBackupsDisabled => OWL_ERR_INCREMENTAL_DISABLED,
            error.AccessDenied => OWL_ERR_STORAGE_ERROR,
            else => OWL_ERR_UNKNOWN,
        };
    };

    if (out_backup_id) |ptr| {
        ptr.* = metadata.backup_id;
    }

    return OWL_OK;
}

/// Perform point-in-time recovery from backup
export fn owl_backup_recover(
    backup_recovery: ?*BackupRecovery,
    backup_id: *const [16]u8,
    target_lsn: u64,
    verify_checksums: bool,
    repair_corruption: bool,
    verbose_logging: bool,
) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;

    const options = RecoveryOptions{
        .target_lsn = target_lsn,
        .verify_checksums = verify_checksums,
        .repair_corruption = repair_corruption,
        .verbose_logging = verbose_logging,
    };

    br.performPointInTimeRecovery(backup_id.*, options) catch |err| {
        return switch (err) {
            error.BackupNotFound => OWL_ERR_BACKUP_NOT_FOUND,
            error.CorruptedBackup => OWL_ERR_BACKUP_CORRUPTED,
            error.BackupChainBroken => OWL_ERR_BACKUP_CHAIN_BROKEN,
            error.ChecksumMismatch => OWL_ERR_CHECKSUM_MISMATCH,
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            else => OWL_ERR_RECOVERY_FAILED,
        };
    };

    return OWL_OK;
}

/// Verify backup integrity
export fn owl_backup_verify(
    backup_recovery: ?*BackupRecovery,
    backup_id: *const [16]u8,
) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;

    const is_valid = br.verifyBackup(backup_id.*) catch |err| {
        return switch (err) {
            error.BackupNotFound => OWL_ERR_BACKUP_NOT_FOUND,
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            else => OWL_ERR_UNKNOWN,
        };
    };

    return if (is_valid) OWL_OK else OWL_ERR_BACKUP_CORRUPTED;
}

/// Delete a backup
export fn owl_backup_delete(
    backup_recovery: ?*BackupRecovery,
    backup_id: *const [16]u8,
) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;

    br.deleteBackup(backup_id.*) catch |err| {
        return switch (err) {
            error.BackupNotFound => OWL_ERR_BACKUP_NOT_FOUND,
            error.AccessDenied => OWL_ERR_STORAGE_ERROR,
            else => OWL_ERR_UNKNOWN,
        };
    };

    return OWL_OK;
}

/// Get backup statistics
export fn owl_backup_get_stats(
    backup_recovery: ?*BackupRecovery,
    out_backups_created: ?*u64,
    out_total_backup_size: ?*u64,
    out_total_compressed_size: ?*u64,
    out_objects_backed_up: ?*u64,
    out_recovery_operations: ?*u64,
    out_objects_recovered: ?*u64,
) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;

    const stats = br.getStats();

    if (out_backups_created) |ptr| ptr.* = stats.backups_created;
    if (out_total_backup_size) |ptr| ptr.* = stats.total_backup_size;
    if (out_total_compressed_size) |ptr| ptr.* = stats.total_compressed_size;
    if (out_objects_backed_up) |ptr| ptr.* = stats.objects_backed_up;
    if (out_recovery_operations) |ptr| ptr.* = stats.recovery_operations;
    if (out_objects_recovered) |ptr| ptr.* = stats.objects_recovered;

    return OWL_OK;
}

/// Cleanup old backups based on retention policy
export fn owl_backup_cleanup_old(backup_recovery: ?*BackupRecovery) c_int {
    const br = backup_recovery orelse return OWL_ERR_INVALID_ARENA;

    br.cleanupOldBackups() catch |err| {
        return switch (err) {
            error.AccessDenied => OWL_ERR_STORAGE_ERROR,
            error.OutOfMemory => OWL_ERR_OUT_OF_MEMORY,
            else => OWL_ERR_UNKNOWN,
        };
    };

    return OWL_OK;
}

// ============================================================================
// Backup and Recovery Tests
// ============================================================================

test "C API: backup and recovery basic operations" {
    // Create test arena
    const arena = owl_arena_create("test_backup_arena.dat", 1024 * 1024);
    defer owl_arena_destroy(arena);
    try std.testing.expect(arena != null);

    // Create test WAL
    const wal_path = "test_backup_wal";
    std.fs.cwd().deleteTree(wal_path) catch {};
    defer std.fs.cwd().deleteTree(wal_path) catch {};

    const wal = gpa.create(WAL) catch return;
    defer gpa.destroy(wal);
    wal.* = WAL.init(gpa, wal_path) catch return;
    defer wal.deinit();

    // Create backup directory
    const backup_dir = "test_backup_dir";
    std.fs.cwd().deleteTree(backup_dir) catch {};
    defer std.fs.cwd().deleteTree(backup_dir) catch {};

    // Create backup recovery manager
    const backup_recovery = owl_backup_recovery_create(arena, wal, backup_dir, 0, 0); // No compression, local storage
    defer owl_backup_recovery_destroy(backup_recovery);
    try std.testing.expect(backup_recovery != null);

    // Create a snapshot backup
    var backup_id: [16]u8 = undefined;
    const result = owl_backup_create_snapshot(backup_recovery, "Test snapshot", &backup_id);
    try std.testing.expectEqual(OWL_OK, result);

    // Verify the backup
    const verify_result = owl_backup_verify(backup_recovery, &backup_id);
    try std.testing.expectEqual(OWL_OK, verify_result);

    // Get backup statistics
    var backups_created: u64 = 0;
    var total_size: u64 = 0;
    const stats_result = owl_backup_get_stats(backup_recovery, &backups_created, &total_size, null, null, null, null);
    try std.testing.expectEqual(OWL_OK, stats_result);
    try std.testing.expectEqual(@as(u64, 1), backups_created);

    // Delete the backup
    const delete_result = owl_backup_delete(backup_recovery, &backup_id);
    try std.testing.expectEqual(OWL_OK, delete_result);
}

test "C API: incremental backup" {
    // Create test arena
    const arena = owl_arena_create("test_incremental_arena.dat", 1024 * 1024);
    defer owl_arena_destroy(arena);
    try std.testing.expect(arena != null);

    // Create test WAL
    const wal_path = "test_incremental_wal";
    std.fs.cwd().deleteTree(wal_path) catch {};
    defer std.fs.cwd().deleteTree(wal_path) catch {};

    const wal = gpa.create(WAL) catch return;
    defer gpa.destroy(wal);
    wal.* = WAL.init(gpa, wal_path) catch return;
    defer wal.deinit();

    // Create backup directory
    const backup_dir = "test_incremental_backup_dir";
    std.fs.cwd().deleteTree(backup_dir) catch {};
    defer std.fs.cwd().deleteTree(backup_dir) catch {};

    // Create backup recovery manager
    const backup_recovery = owl_backup_recovery_create(arena, wal, backup_dir, 1, 0); // LZ4 compression, local storage
    defer owl_backup_recovery_destroy(backup_recovery);
    try std.testing.expect(backup_recovery != null);

    // Create a full backup first
    var full_backup_id: [16]u8 = undefined;
    const full_result = owl_backup_create_full(backup_recovery, "Full backup", &full_backup_id);
    try std.testing.expectEqual(OWL_OK, full_result);

    // Create an incremental backup
    var incremental_backup_id: [16]u8 = undefined;
    const incremental_result = owl_backup_create_incremental(backup_recovery, "Incremental backup", &incremental_backup_id);
    try std.testing.expectEqual(OWL_OK, incremental_result);

    // Verify both backups
    try std.testing.expectEqual(OWL_OK, owl_backup_verify(backup_recovery, &full_backup_id));
    try std.testing.expectEqual(OWL_OK, owl_backup_verify(backup_recovery, &incremental_backup_id));

    // Test recovery from incremental backup
    const recovery_result = owl_backup_recover(backup_recovery, &incremental_backup_id, 0, true, false, false);
    try std.testing.expectEqual(OWL_OK, recovery_result);
}
