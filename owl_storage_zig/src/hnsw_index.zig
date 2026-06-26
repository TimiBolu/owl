const std = @import("std");

const MAGIC: u32 = 0x4853574f; // "OWSH"
const VERSION: u32 = 1;

pub const HnswSearchHit = struct {
    id: u64,
    score: f32,
};

const Node = struct {
    id: u64,
    vector: []f32,
    neighbors: std.ArrayListUnmanaged(u32),
};

pub const HnswIndex = struct {
    allocator: std.mem.Allocator,
    dimension: u32,
    m: u32,
    ef_construction: u32,
    nodes: std.ArrayListUnmanaged(Node),

    pub fn init(
        allocator: std.mem.Allocator,
        dimension: u32,
        m: u32,
        ef_construction: u32,
    ) HnswIndex {
        return .{
            .allocator = allocator,
            .dimension = dimension,
            .m = if (m == 0) 8 else m,
            .ef_construction = if (ef_construction == 0) 64 else ef_construction,
            .nodes = .{},
        };
    }

    pub fn deinit(self: *HnswIndex) void {
        for (self.nodes.items) |*node| {
            self.allocator.free(node.vector);
            node.neighbors.deinit(self.allocator);
        }
        self.nodes.deinit(self.allocator);
    }

    pub fn addVector(self: *HnswIndex, id: u64, vector: []const f32) !void {
        if (vector.len != self.dimension) return error.InvalidDimension;

        const vec_copy = try self.allocator.alloc(f32, vector.len);
        @memcpy(vec_copy, vector);

        const new_index: u32 = @intCast(self.nodes.items.len);
        try self.nodes.append(self.allocator, .{
            .id = id,
            .vector = vec_copy,
            .neighbors = .{},
        });

        if (new_index == 0) return;

        const existing_count = self.nodes.items.len - 1;
        var scored = try self.allocator.alloc(HnswSearchHit, existing_count);
        defer self.allocator.free(scored);

        var i: usize = 0;
        while (i < existing_count) : (i += 1) {
            const score = cosineSimilarity(self.nodes.items[i].vector, vec_copy);
            scored[i] = .{
                .id = @intCast(i),
                .score = score,
            };
        }

        std.mem.sort(HnswSearchHit, scored, {}, struct {
            fn lessThan(_: void, a: HnswSearchHit, b: HnswSearchHit) bool {
                return a.score > b.score;
            }
        }.lessThan);

        const neighbor_count = @min(@as(usize, self.m), scored.len);
        for (0..neighbor_count) |j| {
            const neighbor_idx: u32 = @intCast(scored[j].id);
            try self.nodes.items[new_index].neighbors.append(self.allocator, neighbor_idx);
            try self.nodes.items[neighbor_idx].neighbors.append(self.allocator, new_index);
        }
    }

    pub fn search(
        self: *const HnswIndex,
        query: []const f32,
        k: usize,
        ef: usize,
        allocator: std.mem.Allocator,
    ) ![]HnswSearchHit {
        if (query.len != self.dimension) return error.InvalidDimension;
        if (self.nodes.items.len == 0 or k == 0) return allocator.alloc(HnswSearchHit, 0);

        const entry_idx = self.greedyEntry(query);
        const candidate_limit = @min(@max(ef, k), self.nodes.items.len);

        var visited = try allocator.alloc(bool, self.nodes.items.len);
        defer allocator.free(visited);
        @memset(visited, false);

        var queue = std.ArrayListUnmanaged(u32){};
        defer queue.deinit(allocator);
        try queue.append(allocator, entry_idx);
        visited[entry_idx] = true;

        var cursor: usize = 0;
        while (cursor < queue.items.len and queue.items.len < candidate_limit) : (cursor += 1) {
            const idx = queue.items[cursor];
            const node = self.nodes.items[idx];
            for (node.neighbors.items) |n_idx| {
                if (!visited[n_idx]) {
                    visited[n_idx] = true;
                    try queue.append(allocator, n_idx);
                    if (queue.items.len >= candidate_limit) break;
                }
            }
        }

        var scored = try allocator.alloc(HnswSearchHit, queue.items.len);
        defer allocator.free(scored);
        for (queue.items, 0..) |idx, i| {
            const node = self.nodes.items[idx];
            scored[i] = .{
                .id = node.id,
                .score = cosineSimilarity(node.vector, query),
            };
        }

        std.mem.sort(HnswSearchHit, scored, {}, struct {
            fn lessThan(_: void, a: HnswSearchHit, b: HnswSearchHit) bool {
                return a.score > b.score;
            }
        }.lessThan);

        const topk = @min(k, scored.len);
        const out = try allocator.alloc(HnswSearchHit, topk);
        @memcpy(out, scored[0..topk]);
        return out;
    }

    fn greedyEntry(self: *const HnswIndex, query: []const f32) u32 {
        var current: u32 = 0;
        var best = cosineSimilarity(self.nodes.items[current].vector, query);
        var improved = true;

        while (improved) {
            improved = false;
            for (self.nodes.items[current].neighbors.items) |n_idx| {
                const score = cosineSimilarity(self.nodes.items[n_idx].vector, query);
                if (score > best) {
                    best = score;
                    current = n_idx;
                    improved = true;
                }
            }
        }

        return current;
    }

    pub fn saveToFile(self: *const HnswIndex, path: []const u8) !void {
        var file = try std.fs.cwd().createFile(path, .{ .truncate = true, .read = true });
        defer file.close();
        try writeU32(file, MAGIC);
        try writeU32(file, VERSION);
        try writeU32(file, self.dimension);
        try writeU32(file, self.m);
        try writeU32(file, self.ef_construction);
        try writeU32(file, @intCast(self.nodes.items.len));

        for (self.nodes.items) |node| {
            try writeU64(file, node.id);
            try writeU32(file, @intCast(node.neighbors.items.len));
            for (node.neighbors.items) |n_idx| {
                try writeU32(file, n_idx);
            }
            for (node.vector) |v| {
                try writeU32(file, @bitCast(v));
            }
        }
        try file.sync();
    }

    pub fn loadFromFile(
        allocator: std.mem.Allocator,
        path: []const u8,
    ) !HnswIndex {
        var file = try std.fs.cwd().openFile(path, .{ .mode = .read_only });
        defer file.close();

        const magic = try readU32(file);
        if (magic != MAGIC) return error.InvalidFormat;
        const version = try readU32(file);
        if (version != VERSION) return error.InvalidFormat;

        const dimension = try readU32(file);
        const m = try readU32(file);
        const ef_construction = try readU32(file);
        const node_count = try readU32(file);

        var index = HnswIndex.init(allocator, dimension, m, ef_construction);
        errdefer index.deinit();

        try index.nodes.ensureTotalCapacity(allocator, node_count);
        var i: u32 = 0;
        while (i < node_count) : (i += 1) {
            const id = try readU64(file);
            const neighbor_count = try readU32(file);

            var neighbors = std.ArrayListUnmanaged(u32){};
            try neighbors.ensureTotalCapacity(allocator, neighbor_count);
            var j: u32 = 0;
            while (j < neighbor_count) : (j += 1) {
                try neighbors.append(allocator, try readU32(file));
            }

            var vector = try allocator.alloc(f32, dimension);
            var d: usize = 0;
            while (d < dimension) : (d += 1) {
                const bits = try readU32(file);
                vector[d] = @bitCast(bits);
            }

            try index.nodes.append(allocator, .{
                .id = id,
                .vector = vector,
                .neighbors = neighbors,
            });
        }

        return index;
    }
};

fn writeU32(file: std.fs.File, value: u32) !void {
    var buf: [4]u8 = undefined;
    std.mem.writeInt(u32, &buf, value, .little);
    try file.writeAll(&buf);
}

fn writeU64(file: std.fs.File, value: u64) !void {
    var buf: [8]u8 = undefined;
    std.mem.writeInt(u64, &buf, value, .little);
    try file.writeAll(&buf);
}

fn readU32(file: std.fs.File) !u32 {
    var buf: [4]u8 = undefined;
    try readExact(file, &buf);
    return std.mem.readInt(u32, &buf, .little);
}

fn readU64(file: std.fs.File) !u64 {
    var buf: [8]u8 = undefined;
    try readExact(file, &buf);
    return std.mem.readInt(u64, &buf, .little);
}

fn readExact(file: std.fs.File, out: []u8) !void {
    var offset: usize = 0;
    while (offset < out.len) {
        const n = try file.read(out[offset..]);
        if (n == 0) return error.EndOfStream;
        offset += n;
    }
}

fn cosineSimilarity(a: []const f32, b: []const f32) f32 {
    var dot: f64 = 0;
    var norm_a: f64 = 0;
    var norm_b: f64 = 0;
    for (a, b) |av, bv| {
        const af = @as(f64, av);
        const bf = @as(f64, bv);
        dot += af * bf;
        norm_a += af * af;
        norm_b += bf * bf;
    }
    if (norm_a == 0 or norm_b == 0) return 0.0;
    return @floatCast(dot / (std.math.sqrt(norm_a) * std.math.sqrt(norm_b)));
}

test "hnsw save/load and search" {
    const allocator = std.testing.allocator;
    var index = HnswIndex.init(allocator, 3, 4, 16);
    defer index.deinit();

    try index.addVector(1, &[_]f32{ 1.0, 0.0, 0.0 });
    try index.addVector(2, &[_]f32{ 0.0, 1.0, 0.0 });
    try index.addVector(3, &[_]f32{ 0.9, 0.1, 0.0 });

    const path = "/tmp/owl_hnsw_test.idx";
    _ = std.fs.cwd().deleteFile(path) catch {};
    try index.saveToFile(path);

    var loaded = try HnswIndex.loadFromFile(allocator, path);
    defer loaded.deinit();

    const hits = try loaded.search(&[_]f32{ 1.0, 0.0, 0.0 }, 2, 8, allocator);
    defer allocator.free(hits);
    try std.testing.expect(hits.len == 2);
}
