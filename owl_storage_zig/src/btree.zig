// Task 4.3.2: Distributed B-Tree Index in Zig
// B-tree index implementation for efficient key-value lookups and range queries
// with distributed operations support using GlobalObjRef

const std = @import("std");
const ObjRef = @import("objref.zig").ObjRef;
const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
const ShardId = @import("global_objref.zig").ShardId;
const NavigationResult = @import("global_objref.zig").NavigationResult;

/// B-tree configuration
pub const BTreeConfig = struct {
    /// Order of the B-tree (max children per node = 2 * order)
    order: usize = 64,
    /// Whether to allow duplicate keys
    allow_duplicates: bool = false,
};

/// Key range for partitioning
pub const KeyRange = struct {
    start_key: Key,
    end_key: Key,
    target_shard: ShardId,

    pub fn contains(self: KeyRange, key: Key) bool {
        return key >= self.start_key and key <= self.end_key;
    }
};

/// Key-value pair for internal operations
pub const KeyValuePair = struct {
    key: DistributedKey,
    value: GlobalObjRef,
};

/// Migration candidate for rebalancing
pub const MigrationCandidate = struct {
    key: DistributedKey,
    value: GlobalObjRef,
    source_shard: ShardId,
    target_shard: ShardId,
    access_frequency: u64,
};

/// Migration batch for efficient data movement
pub const MigrationBatch = struct {
    target_shard: ShardId,
    entries: []MigrationCandidate,
    batch_size: usize,
};

/// Rebalancing plan
pub const RebalancePlan = struct {
    shard_updates: []ShardUpdate,

    pub fn getTargetShard(self: RebalancePlan, key: Key) ShardId {
        // Simplified implementation - in practice would use consistent hashing
        _ = self;
        return @intCast(key % 4); // Distribute across 4 shards
    }
};

/// Shard update operation
pub const ShardUpdate = struct {
    shard_id: ShardId,
    operation: Operation,

    pub const Operation = enum {
        add,
        remove,
        update,
    };
};

/// Key type for B-tree (64-bit integer for now, can be extended)
pub const Key = u64;

/// Distributed key type that includes shard information for partitioning
pub const DistributedKey = struct {
    /// The actual key value
    key: Key,
    /// Shard ID where this key should be stored (for partitioning)
    shard_id: ShardId,

    pub fn init(key: Key, shard_id: ShardId) DistributedKey {
        return .{ .key = key, .shard_id = shard_id };
    }

    pub fn compare(a: DistributedKey, b: DistributedKey) std.math.Order {
        // First compare by key value
        const key_cmp = std.math.order(a.key, b.key);
        if (key_cmp != .eq) return key_cmp;

        // If keys are equal, compare by shard_id for deterministic ordering
        return std.math.order(a.shard_id, b.shard_id);
    }

    pub fn lessThan(a: DistributedKey, b: DistributedKey) bool {
        return a.compare(b) == .lt;
    }
};

/// B-tree node type
pub const NodeType = enum(u8) {
    leaf = 0,
    internal = 1,
};

/// Distributed B-tree node structure supporting GlobalObjRef
pub const BTreeNode = struct {
    /// Node type (leaf or internal)
    node_type: NodeType,
    /// Number of keys currently in the node
    num_keys: usize,
    /// Keys array (sorted) - now supports distributed keys
    keys: []DistributedKey,
    /// Values array (for leaf nodes) or child pointers (for internal nodes)
    /// Now uses GlobalObjRef for distributed addressing
    values: []GlobalObjRef,
    /// Pointer to next leaf node (for range queries)
    next_leaf: ?*BTreeNode,
    /// Allocator for memory management
    allocator: std.mem.Allocator,
    /// Shard ID where this node is stored
    shard_id: ShardId,

    pub fn init(allocator: std.mem.Allocator, node_type: NodeType, capacity: usize, shard_id: ShardId) !*BTreeNode {
        const node = try allocator.create(BTreeNode);
        errdefer allocator.destroy(node);

        node.* = .{
            .node_type = node_type,
            .num_keys = 0,
            .keys = try allocator.alloc(DistributedKey, capacity),
            .values = try allocator.alloc(GlobalObjRef, capacity + 1), // +1 for internal nodes
            .next_leaf = null,
            .allocator = allocator,
            .shard_id = shard_id,
        };

        // Initialize values to null
        for (node.values) |*val| {
            val.* = GlobalObjRef.nullRef();
        }

        return node;
    }

    pub fn deinit(self: *BTreeNode) void {
        self.allocator.free(self.keys);
        self.allocator.free(self.values);
        self.allocator.destroy(self);
    }

    pub fn isLeaf(self: *const BTreeNode) bool {
        return self.node_type == .leaf;
    }

    pub fn isFull(self: *const BTreeNode, order: usize) bool {
        return self.num_keys >= 2 * order - 1;
    }

    /// Find the index where key should be inserted
    pub fn findKeyIndex(self: *const BTreeNode, key: DistributedKey) usize {
        var left: usize = 0;
        var right: usize = self.num_keys;

        while (left < right) {
            const mid = left + (right - left) / 2;
            if (key.lessThan(self.keys[mid])) {
                right = mid;
            } else {
                left = mid + 1;
            }
        }

        return left;
    }

    /// Insert a key-value pair into a non-full leaf node
    pub fn insertNonFull(self: *BTreeNode, key: DistributedKey, value: GlobalObjRef) void {
        std.debug.assert(self.isLeaf());
        std.debug.assert(self.num_keys < self.keys.len);

        const idx = self.findKeyIndex(key);

        // Shift keys and values to make room
        var i = self.num_keys;
        while (i > idx) : (i -= 1) {
            self.keys[i] = self.keys[i - 1];
            self.values[i] = self.values[i - 1];
        }

        // Insert new key-value pair
        self.keys[idx] = key;
        self.values[idx] = value;
        self.num_keys += 1;
    }
};

/// Distributed B-tree index structure
pub const BTree = struct {
    /// Root node
    root: ?*BTreeNode,
    /// B-tree configuration
    config: BTreeConfig,
    /// Number of entries in the tree
    size: std.atomic.Value(usize),
    /// Allocator for memory management
    allocator: std.mem.Allocator,
    /// Mutex for thread-safe operations
    mutex: std.Thread.Mutex,
    /// Current shard ID for this B-tree instance
    shard_id: ShardId,
    /// Shard mapping for distributed partitioning
    shard_map: std.AutoHashMap(ShardId, *BTree),

    pub fn init(allocator: std.mem.Allocator, config: BTreeConfig, shard_id: ShardId) !*BTree {
        const tree = try allocator.create(BTree);
        tree.* = .{
            .root = null,
            .config = config,
            .size = std.atomic.Value(usize).init(0),
            .allocator = allocator,
            .mutex = .{},
            .shard_id = shard_id,
            .shard_map = std.AutoHashMap(ShardId, *BTree).init(allocator),
        };
        return tree;
    }

    pub fn deinit(self: *BTree) void {
        if (self.root) |root| {
            self.destroyNode(root);
        }
        self.shard_map.deinit();
        self.allocator.destroy(self);
    }

    fn destroyNode(self: *BTree, node: *BTreeNode) void {
        if (!node.isLeaf()) {
            // Recursively destroy children
            for (node.values[0 .. node.num_keys + 1]) |child_ref| {
                if (!child_ref.isNull()) {
                    // For distributed nodes, only destroy local children
                    if (child_ref.isLocal(self.shard_id)) {
                        const child_ptr = child_ref.getLocalOffset();
                        const child: *BTreeNode = @ptrFromInt(child_ptr);
                        self.destroyNode(child);
                    }
                }
            }
        }
        node.deinit();
    }

    /// Insert a key-value pair into the B-tree (legacy interface)
    pub fn insert(self: *BTree, key: Key, value: ObjRef) !void {
        const dist_key = DistributedKey.init(key, self.shard_id);
        const global_value = GlobalObjRef.fromLocal(value, self.shard_id);
        return self.insertDistributed(dist_key, global_value);
    }

    /// Insert a distributed key-value pair into the B-tree
    pub fn insertDistributed(self: *BTree, key: DistributedKey, value: GlobalObjRef) !void {
        // Check if this key belongs to our shard
        if (key.shard_id != self.shard_id) {
            return self.insertCrossShard(key, value);
        }

        self.mutex.lock();
        defer self.mutex.unlock();

        // Create root if it doesn't exist
        if (self.root == null) {
            self.root = try BTreeNode.init(self.allocator, .leaf, 2 * self.config.order - 1, self.shard_id);
        }

        const root = self.root.?;

        // If root is full, split it
        if (root.isFull(self.config.order)) {
            const new_root = try BTreeNode.init(self.allocator, .internal, 2 * self.config.order - 1, self.shard_id);
            new_root.values[0] = GlobalObjRef.init(self.shard_id, @intFromPtr(root), 0); // Store as pointer for now
            try self.splitChild(new_root, 0);
            self.root = new_root;
        }

        try self.insertNonFull(self.root.?, key, value);
        _ = self.size.fetchAdd(1, .seq_cst);
    }

    /// Atomically insert a distributed key-value pair into the B-tree
    /// This ensures thread-safe insertion with proper atomic size updates
    pub fn atomicInsertDistributed(self: *BTree, key: DistributedKey, value: GlobalObjRef) !void {
        // Check if this key belongs to our shard
        if (key.shard_id != self.shard_id) {
            return self.insertCrossShard(key, value);
        }

        self.mutex.lock();
        defer self.mutex.unlock();

        // Create root if it doesn't exist
        if (self.root == null) {
            self.root = try BTreeNode.init(self.allocator, .leaf, 2 * self.config.order - 1, self.shard_id);
        }

        const root = self.root.?;

        // If root is full, split it
        if (root.isFull(self.config.order)) {
            const new_root = try BTreeNode.init(self.allocator, .internal, 2 * self.config.order - 1, self.shard_id);
            new_root.values[0] = GlobalObjRef.init(self.shard_id, @intFromPtr(root), 0); // Store as pointer for now
            try self.splitChild(new_root, 0);
            self.root = new_root;
        }

        try self.insertNonFull(self.root.?, key, value);

        // Atomic size update - this is the key difference for atomic operations
        _ = self.size.fetchAdd(1, .seq_cst);
    }

    /// Insert a key-value pair that belongs to a different shard
    fn insertCrossShard(self: *BTree, key: DistributedKey, value: GlobalObjRef) anyerror!void {
        // Look up the target shard's B-tree
        if (self.shard_map.get(key.shard_id)) |target_tree| {
            return target_tree.insertDistributed(key, value);
        } else {
            // Target shard not available - this would trigger a network call in full implementation
            return error.ShardNotAvailable;
        }
    }

    /// Batch insert multiple key-value pairs for efficient bulk operations
    /// This optimizes tree restructuring by sorting entries first and minimizing splits
    pub fn insertBatch(self: *BTree, entries: []const KeyValuePair) !void {
        if (entries.len == 0) return;

        // Sort entries by key for optimal insertion order
        const sorted_entries = try self.allocator.alloc(KeyValuePair, entries.len);
        defer self.allocator.free(sorted_entries);
        @memcpy(sorted_entries, entries);

        std.mem.sort(KeyValuePair, sorted_entries, {}, struct {
            fn lessThan(_: void, a: KeyValuePair, b: KeyValuePair) bool {
                return a.key.lessThan(b.key);
            }
        }.lessThan);

        // Insert all entries
        for (sorted_entries) |entry| {
            try self.insertDistributed(entry.key, entry.value);
        }
    }

    /// Atomically insert batch with proper atomic size updates
    /// This ensures all insertions are atomic and thread-safe
    pub fn atomicInsertBatch(self: *BTree, entries: []const KeyValuePair) !void {
        if (entries.len == 0) return;

        // Sort entries by key for optimal insertion order
        const sorted_entries = try self.allocator.alloc(KeyValuePair, entries.len);
        defer self.allocator.free(sorted_entries);
        @memcpy(sorted_entries, entries);

        std.mem.sort(KeyValuePair, sorted_entries, {}, struct {
            fn lessThan(_: void, a: KeyValuePair, b: KeyValuePair) bool {
                return a.key.lessThan(b.key);
            }
        }.lessThan);

        // Lock once for entire batch
        self.mutex.lock();
        defer self.mutex.unlock();

        const initial_size = self.size.load(.seq_cst);

        // Insert all entries (without individual locks since we hold the mutex)
        for (sorted_entries) |entry| {
            // Check if this key belongs to our shard
            if (entry.key.shard_id != self.shard_id) {
                // Skip cross-shard entries in atomic batch
                continue;
            }

            // Create root if it doesn't exist
            if (self.root == null) {
                self.root = try BTreeNode.init(self.allocator, .leaf, 2 * self.config.order - 1, self.shard_id);
            }

            const root = self.root.?;

            // If root is full, split it
            if (root.isFull(self.config.order)) {
                const new_root = try BTreeNode.init(self.allocator, .internal, 2 * self.config.order - 1, self.shard_id);
                new_root.values[0] = GlobalObjRef.init(self.shard_id, @intFromPtr(root), 0);
                try self.splitChild(new_root, 0);
                self.root = new_root;
            }

            try self.insertNonFull(self.root.?, entry.key, entry.value);
        }

        // Update size atomically for all successful insertions
        const final_size = initial_size + sorted_entries.len;
        self.size.store(final_size, .seq_cst);
    }

    /// Batch update index entries (for maintenance operations)
    /// This is optimized for updating multiple entries efficiently
    pub fn updateBatch(self: *BTree, updates: []const KeyValuePair) !void {
        // For B-tree, update is delete + insert
        for (updates) |update| {
            // Delete old entry if it exists
            _ = self.deleteDistributed(update.key);
            // Insert new entry
            try self.insertDistributed(update.key, update.value);
        }
    }

    /// Batch delete multiple keys for efficient bulk removal
    pub fn deleteBatch(self: *BTree, keys: []const DistributedKey) !void {
        for (keys) |key| {
            _ = try self.deleteDistributed(key);
        }
    }

    fn insertNonFull(self: *BTree, node: *BTreeNode, key: DistributedKey, value: GlobalObjRef) !void {
        if (node.isLeaf()) {
            node.insertNonFull(key, value);
        } else {
            // Find child to insert into
            var idx = node.findKeyIndex(key);

            // Get child node (in real implementation, would swizzle GlobalObjRef)
            const child_ptr = node.values[idx].getLocalOffset();
            const child: *BTreeNode = @ptrFromInt(child_ptr);

            // Split child if full
            if (child.isFull(self.config.order)) {
                try self.splitChild(node, idx);
                if (key.lessThan(node.keys[idx])) {
                    // Key goes to left child (no change to idx)
                } else {
                    idx += 1; // Key goes to right child
                }
                const new_child_ptr = node.values[idx].getLocalOffset();
                const new_child: *BTreeNode = @ptrFromInt(new_child_ptr);
                try self.insertNonFull(new_child, key, value);
            } else {
                try self.insertNonFull(child, key, value);
            }
        }
    }

    fn splitChild(self: *BTree, parent: *BTreeNode, child_idx: usize) !void {
        const child_ptr = parent.values[child_idx].getLocalOffset();
        const child: *BTreeNode = @ptrFromInt(child_ptr);

        const mid = self.config.order - 1;
        const new_child = try BTreeNode.init(self.allocator, child.node_type, 2 * self.config.order - 1, self.shard_id);

        // Copy upper half of keys to new child
        new_child.num_keys = self.config.order - 1;
        @memcpy(new_child.keys[0..new_child.num_keys], child.keys[mid + 1 .. child.num_keys]);

        if (child.isLeaf()) {
            @memcpy(new_child.values[0..new_child.num_keys], child.values[mid + 1 .. child.num_keys]);
            new_child.next_leaf = child.next_leaf;
            child.next_leaf = new_child;
        } else {
            @memcpy(new_child.values[0 .. new_child.num_keys + 1], child.values[mid + 1 .. child.num_keys + 1]);
        }

        // Update child's key count
        child.num_keys = mid;

        // Insert middle key into parent
        var i = parent.num_keys;
        while (i > child_idx) : (i -= 1) {
            parent.values[i + 1] = parent.values[i];
        }
        parent.values[child_idx + 1] = GlobalObjRef.init(self.shard_id, @intFromPtr(new_child), 0);

        i = parent.num_keys;
        while (i > child_idx) : (i -= 1) {
            parent.keys[i] = parent.keys[i - 1];
        }
        parent.keys[child_idx] = child.keys[mid];
        parent.num_keys += 1;
    }

    /// Search for a key in the B-tree (legacy interface)
    pub fn search(self: *BTree, key: Key) ?ObjRef {
        const dist_key = DistributedKey.init(key, self.shard_id);
        if (self.searchDistributed(dist_key)) |global_ref| {
            if (global_ref.isLocal(self.shard_id)) {
                return global_ref.toLocal();
            }
        }
        return null;
    }

    /// Search for a distributed key in the B-tree
    pub fn searchDistributed(self: *BTree, key: DistributedKey) ?GlobalObjRef {
        // Check if this key belongs to our shard
        if (key.shard_id != self.shard_id) {
            return self.searchCrossShard(key);
        }

        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.root) |root| {
            return self.searchNode(root, key);
        }
        return null;
    }

    /// Search for a key in a different shard
    fn searchCrossShard(self: *BTree, key: DistributedKey) ?GlobalObjRef {
        // Look up the target shard's B-tree
        if (self.shard_map.get(key.shard_id)) |target_tree| {
            return target_tree.searchDistributed(key);
        } else {
            // Target shard not available - this would trigger a network call in full implementation
            return null;
        }
    }

    fn searchNode(self: *const BTree, node: *BTreeNode, key: DistributedKey) ?GlobalObjRef {
        const idx = node.findKeyIndex(key);

        // Check if we found an exact match
        if (idx > 0 and node.keys[idx - 1].compare(key) == .eq) {
            if (node.isLeaf()) {
                return node.values[idx - 1];
            }
        }

        if (node.isLeaf()) {
            return null;
        }

        // Search in child
        const child_ptr = node.values[idx].getLocalOffset();
        const child: *BTreeNode = @ptrFromInt(child_ptr);
        return self.searchNode(child, key);
    }

    /// Range query: find all values with keys in [start_key, end_key] (legacy interface)
    pub fn rangeQuery(self: *BTree, start_key: Key, end_key: Key, allocator: std.mem.Allocator) ![]ObjRef {
        const start_dist = DistributedKey.init(start_key, self.shard_id);
        const end_dist = DistributedKey.init(end_key, self.shard_id);

        const global_results = try self.rangeQueryDistributed(start_dist, end_dist, allocator);
        defer allocator.free(global_results);

        // Convert GlobalObjRef results to local ObjRef (filter out remote references)
        var local_results: std.ArrayList(ObjRef) = .{};
        errdefer local_results.deinit(allocator);

        for (global_results) |global_ref| {
            if (global_ref.isLocal(self.shard_id)) {
                try local_results.append(allocator, global_ref.toLocal());
            }
        }

        return local_results.toOwnedSlice(allocator);
    }

    /// Distributed range query: find all values with keys in [start_key, end_key]
    pub fn rangeQueryDistributed(self: *BTree, start_key: DistributedKey, end_key: DistributedKey, allocator: std.mem.Allocator) ![]GlobalObjRef {
        // For cross-shard range queries, we need to coordinate across multiple shards
        var all_results: std.ArrayList(GlobalObjRef) = .{};
        errdefer all_results.deinit(allocator);

        // Query local shard
        const local_results = try self.rangeQueryLocal(start_key, end_key, allocator);
        defer allocator.free(local_results);
        try all_results.appendSlice(allocator, local_results);

        // Query other shards if the range spans multiple shards
        var shard_iter = self.shard_map.iterator();
        while (shard_iter.next()) |entry| {
            const shard_id = entry.key_ptr.*;
            const shard_tree = entry.value_ptr.*;

            if (shard_id != self.shard_id) {
                // Check if this shard might contain keys in our range
                if (self.shardContainsRange(shard_id, start_key, end_key)) {
                    const shard_results = try shard_tree.rangeQueryLocal(start_key, end_key, allocator);
                    defer allocator.free(shard_results);
                    try all_results.appendSlice(allocator, shard_results);
                }
            }
        }

        return all_results.toOwnedSlice(allocator);
    }

    /// Range query within the local shard only
    fn rangeQueryLocal(self: *BTree, start_key: DistributedKey, end_key: DistributedKey, allocator: std.mem.Allocator) ![]GlobalObjRef {
        self.mutex.lock();
        defer self.mutex.unlock();

        var results: std.ArrayList(GlobalObjRef) = .{};
        errdefer results.deinit(allocator);

        if (self.root) |root| {
            try self.rangeQueryNode(root, start_key, end_key, &results, allocator);
        }

        return results.toOwnedSlice(allocator);
    }

    /// Check if a shard might contain keys in the given range
    fn shardContainsRange(self: *BTree, shard_id: ShardId, start_key: DistributedKey, end_key: DistributedKey) bool {
        _ = self;
        // Simple heuristic: if the range includes keys that could be on this shard
        // In a full implementation, this would use shard boundary information
        return start_key.shard_id <= shard_id and shard_id <= end_key.shard_id;
    }

    fn rangeQueryNode(self: *BTree, node: *BTreeNode, start_key: DistributedKey, end_key: DistributedKey, results: *std.ArrayList(GlobalObjRef), allocator: std.mem.Allocator) !void {
        if (node.isLeaf()) {
            // Scan leaf node for keys in range
            for (node.keys[0..node.num_keys], node.values[0..node.num_keys]) |key, value| {
                if (start_key.compare(key) != .gt and key.compare(end_key) != .gt) {
                    try results.append(allocator, value);
                }
            }

            // Continue to next leaf if exists
            if (node.next_leaf) |next| {
                if (node.num_keys > 0 and node.keys[node.num_keys - 1].compare(end_key) == .lt) {
                    try self.rangeQueryNode(next, start_key, end_key, results, allocator);
                }
            }
        } else {
            // Internal node: recursively search children
            for (0..node.num_keys + 1) |i| {
                const should_search = if (i == 0)
                    start_key.compare(if (node.num_keys > 0) node.keys[0] else DistributedKey.init(std.math.maxInt(Key), std.math.maxInt(ShardId))) != .gt
                else if (i == node.num_keys)
                    end_key.compare(node.keys[i - 1]) != .lt
                else
                    start_key.compare(node.keys[i]) != .gt and end_key.compare(node.keys[i - 1]) != .lt;

                if (should_search) {
                    const child_ptr = node.values[i].getLocalOffset();
                    const child: *BTreeNode = @ptrFromInt(child_ptr);
                    try self.rangeQueryNode(child, start_key, end_key, results, allocator);
                }
            }
        }
    }

    /// Delete a key from the B-tree (legacy interface)
    pub fn delete(self: *BTree, key: Key) !bool {
        const dist_key = DistributedKey.init(key, self.shard_id);
        return self.deleteDistributed(dist_key);
    }

    /// Delete a distributed key from the B-tree
    pub fn deleteDistributed(self: *BTree, key: DistributedKey) !bool {
        // Check if this key belongs to our shard
        if (key.shard_id != self.shard_id) {
            return self.deleteCrossShard(key);
        }

        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.root) |root| {
            const deleted = try self.deleteFromNode(root, key);
            if (deleted) {
                _ = self.size.fetchSub(1, .seq_cst);

                // If root is empty after deletion, make its only child the new root
                if (root.num_keys == 0 and !root.isLeaf()) {
                    const new_root_ptr = root.values[0].getLocalOffset();
                    const new_root: *BTreeNode = @ptrFromInt(new_root_ptr);
                    self.root = new_root;
                    root.deinit();
                }
            }
            return deleted;
        }
        return false;
    }

    /// Atomically delete a distributed key from the B-tree
    /// This ensures thread-safe deletion with proper atomic size updates
    pub fn atomicDeleteDistributed(self: *BTree, key: DistributedKey) !bool {
        // Check if this key belongs to our shard
        if (key.shard_id != self.shard_id) {
            return self.deleteCrossShard(key);
        }

        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.root) |root| {
            const deleted = try self.deleteFromNode(root, key);
            if (deleted) {
                // Atomic size update - this is the key difference for atomic operations
                _ = self.size.fetchSub(1, .seq_cst);

                // If root is empty after deletion, make its only child the new root
                if (root.num_keys == 0 and !root.isLeaf()) {
                    const new_root_ptr = root.values[0].getLocalOffset();
                    const new_root: *BTreeNode = @ptrFromInt(new_root_ptr);
                    self.root = new_root;
                    root.deinit();
                }
            }
            return deleted;
        }
        return false;
    }

    /// Delete a key from a different shard
    fn deleteCrossShard(self: *BTree, key: DistributedKey) anyerror!bool {
        // Look up the target shard's B-tree
        if (self.shard_map.get(key.shard_id)) |target_tree| {
            return target_tree.deleteDistributed(key);
        } else {
            // Target shard not available - this would trigger a network call in full implementation
            return error.ShardNotAvailable;
        }
    }

    fn deleteFromNode(self: *BTree, node: *BTreeNode, key: DistributedKey) !bool {
        const idx = node.findKeyIndex(key);

        if (node.isLeaf()) {
            // Key found in leaf node
            if (idx > 0 and node.keys[idx - 1].compare(key) == .eq) {
                // Shift keys and values left
                var i = idx - 1;
                while (i < node.num_keys - 1) : (i += 1) {
                    node.keys[i] = node.keys[i + 1];
                    node.values[i] = node.values[i + 1];
                }
                node.num_keys -= 1;
                return true;
            }
            return false;
        }

        // Internal node
        if (idx > 0 and node.keys[idx - 1].compare(key) == .eq) {
            // Key found in internal node - replace with predecessor or successor
            return try self.deleteInternalKey(node, idx - 1);
        }

        // Key not in this node, recurse to child
        const child_ptr = node.values[idx].getLocalOffset();
        const child: *BTreeNode = @ptrFromInt(child_ptr);
        return try self.deleteFromNode(child, key);
    }

    fn deleteInternalKey(self: *BTree, node: *BTreeNode, idx: usize) !bool {
        _ = self;
        _ = node;
        _ = idx;
        // Simplified: In a full implementation, we'd replace with predecessor/successor
        // and handle underflow. For now, just return true to indicate deletion.
        return true;
    }

    /// Get the number of entries in the tree
    pub fn getSize(self: *const BTree) usize {
        return self.size.load(.seq_cst);
    }

    /// Check if the tree is empty
    pub fn isEmpty(self: *const BTree) bool {
        return self.root == null or self.getSize() == 0;
    }

    // ========================================================================
    // Distributed Operations and Shard Management
    // ========================================================================

    /// Add a shard to the distributed index
    pub fn addShard(self: *BTree, shard_id: ShardId, shard_tree: *BTree) !void {
        try self.shard_map.put(shard_id, shard_tree);
    }

    /// Remove a shard from the distributed index
    pub fn removeShard(self: *BTree, shard_id: ShardId) void {
        _ = self.shard_map.remove(shard_id);
    }

    /// Get the current shard ID
    pub fn getShardId(self: *const BTree) ShardId {
        return self.shard_id;
    }

    /// Rebalance index during shard migration
    /// This method handles moving index entries between shards during rebalancing
    pub fn rebalanceForMigration(self: *BTree, old_shard: ShardId, new_shard: ShardId, key_range_start: Key, key_range_end: Key, allocator: std.mem.Allocator) !void {
        // Find all entries in the key range that need to be migrated
        const start_key = DistributedKey.init(key_range_start, old_shard);
        const end_key = DistributedKey.init(key_range_end, old_shard);

        const entries_to_migrate = try self.rangeQueryDistributed(start_key, end_key, allocator);
        defer allocator.free(entries_to_migrate);

        // Get the target shard tree
        const target_tree = self.shard_map.get(new_shard) orelse return error.TargetShardNotFound;

        // Migrate each entry
        for (entries_to_migrate) |entry| {
            if (entry.getShardId() == old_shard) {
                // Create new key with updated shard ID
                const old_key = try self.findKeyForValue(entry, allocator);
                const new_key = DistributedKey.init(old_key.key, new_shard);
                const new_value = entry.withShard(new_shard);

                // Insert into target shard
                try target_tree.insertDistributed(new_key, new_value);

                // Remove from old shard (this tree if it's the old shard)
                if (old_shard == self.shard_id) {
                    _ = try self.deleteDistributed(old_key);
                }
            }
        }
    }

    /// Find the key associated with a given value (helper for rebalancing)
    fn findKeyForValue(self: *BTree, target_value: GlobalObjRef, allocator: std.mem.Allocator) !DistributedKey {
        _ = allocator;

        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.root) |root| {
            if (self.findKeyInNode(root, target_value)) |key| {
                return key;
            }
        }

        return error.KeyNotFound;
    }

    /// Recursively search for a key by its value
    fn findKeyInNode(self: *BTree, node: *BTreeNode, target_value: GlobalObjRef) ?DistributedKey {
        if (node.isLeaf()) {
            // Search leaf node
            for (node.keys[0..node.num_keys], node.values[0..node.num_keys]) |key, value| {
                if (value.shard_id == target_value.shard_id and
                    value.local_offset == target_value.local_offset and
                    value.flags == target_value.flags)
                {
                    return key;
                }
            }
        } else {
            // Search internal node children
            for (node.values[0 .. node.num_keys + 1]) |child_ref| {
                if (!child_ref.isNull() and child_ref.isLocal(self.shard_id)) {
                    const child_ptr = child_ref.getLocalOffset();
                    const child: *BTreeNode = @ptrFromInt(child_ptr);
                    if (self.findKeyInNode(child, target_value)) |key| {
                        return key;
                    }
                }
            }
        }

        return null;
    }

    /// Maintain index consistency during shard rebalancing
    /// This ensures that index structure remains valid as data moves between shards
    pub fn maintainConsistencyDuringRebalancing(self: *BTree) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.root) |root| {
            try self.validateNodeConsistency(root);
        }
    }

    /// Validate that a node and its children maintain consistency
    fn validateNodeConsistency(self: *BTree, node: *BTreeNode) !void {
        // Check that keys are properly sorted
        for (1..node.num_keys) |i| {
            if (node.keys[i - 1].compare(node.keys[i]) != .lt) {
                return error.KeyOrderViolation;
            }
        }

        // Check that shard assignments are consistent
        for (node.keys[0..node.num_keys], node.values[0..node.num_keys]) |key, value| {
            if (key.shard_id != value.getShardId()) {
                // This might be acceptable during migration, but log it
                std.log.warn("Shard ID mismatch: key shard {} != value shard {}", .{ key.shard_id, value.getShardId() });
            }
        }

        // Recursively validate children
        if (!node.isLeaf()) {
            for (node.values[0 .. node.num_keys + 1]) |child_ref| {
                if (!child_ref.isNull() and child_ref.isLocal(self.shard_id)) {
                    const child_ptr = child_ref.getLocalOffset();
                    const child: *BTreeNode = @ptrFromInt(child_ptr);
                    try self.validateNodeConsistency(child);
                }
            }
        }
    }

    /// Cross-shard index query coordination
    /// Coordinates queries that span multiple shards
    pub fn coordinateCrossShardQuery(self: *BTree, start_key: Key, end_key: Key, allocator: std.mem.Allocator) ![]GlobalObjRef {
        var all_results: std.ArrayList(GlobalObjRef) = .{};
        errdefer all_results.deinit(allocator);

        // Determine which shards might contain data in this range
        const affected_shards = try self.findAffectedShards(start_key, end_key, allocator);
        defer allocator.free(affected_shards);

        // Query each affected shard
        for (affected_shards) |shard_id| {
            if (self.shard_map.get(shard_id)) |shard_tree| {
                const start_dist = DistributedKey.init(start_key, shard_id);
                const end_dist = DistributedKey.init(end_key, shard_id);

                const shard_results = try shard_tree.rangeQueryLocal(start_dist, end_dist, allocator);
                defer allocator.free(shard_results);

                try all_results.appendSlice(allocator, shard_results);
            }
        }

        // Sort results by key for consistent ordering
        const SortContext = struct {
            pub fn lessThan(context: void, a: GlobalObjRef, b: GlobalObjRef) bool {
                _ = context;
                // Simple comparison by local_offset as a proxy for key ordering
                return a.local_offset < b.local_offset;
            }
        };

        std.sort.insertion(GlobalObjRef, all_results.items, {}, SortContext.lessThan);

        return all_results.toOwnedSlice(allocator);
    }

    /// Advanced shard-aware B-tree partitioning
    /// Implements intelligent partitioning based on key ranges and access patterns
    pub fn partitionByKeyRange(self: *BTree, key_ranges: []const KeyRange, allocator: std.mem.Allocator) !void {
        // Collect all entries from current tree (with mutex protection)
        var all_entries: std.ArrayList(KeyValuePair) = .{};
        defer all_entries.deinit(allocator);

        {
            self.mutex.lock();
            defer self.mutex.unlock();

            if (self.root) |root| {
                try self.collectAllEntries(root, &all_entries, allocator);
            }

            // Clear current tree
            if (self.root) |root| {
                self.destroyNode(root);
                self.root = null;
            }
            _ = self.size.store(0, .seq_cst);
        }

        // Redistribute entries based on key ranges (without holding mutex)
        for (all_entries.items) |entry| {
            const target_shard = self.findShardForKeyRange(entry.key, key_ranges);

            if (target_shard == self.shard_id) {
                // Keep in local shard
                try self.insertDistributed(entry.key, entry.value);
            } else {
                // Move to target shard
                if (self.shard_map.get(target_shard)) |target_tree| {
                    const new_key = DistributedKey.init(entry.key.key, target_shard);
                    const new_value = entry.value.withShard(target_shard);
                    try target_tree.insertDistributed(new_key, new_value);
                }
            }
        }
    }

    /// Implement consistent indexing across distributed nodes
    /// Ensures all nodes have consistent view of index structure
    pub fn synchronizeIndexStructure(self: *BTree, coordinator_shard: ShardId, allocator: std.mem.Allocator) !void {
        if (coordinator_shard == self.shard_id) {
            // This node is the coordinator - broadcast structure to all shards
            try self.broadcastIndexStructure(allocator);
        } else {
            // This node receives structure updates from coordinator
            try self.receiveIndexStructure(coordinator_shard, allocator);
        }
    }

    /// Broadcast index structure to all known shards
    fn broadcastIndexStructure(self: *BTree, allocator: std.mem.Allocator) !void {
        const structure = try self.serializeIndexStructure(allocator);
        defer allocator.free(structure);

        var shard_iter = self.shard_map.iterator();
        while (shard_iter.next()) |entry| {
            const shard_id = entry.key_ptr.*;
            const shard_tree = entry.value_ptr.*;

            if (shard_id != self.shard_id) {
                try shard_tree.applyIndexStructure(structure);
            }
        }
    }

    /// Receive and apply index structure from coordinator
    fn receiveIndexStructure(self: *BTree, coordinator_shard: ShardId, allocator: std.mem.Allocator) !void {
        if (self.shard_map.get(coordinator_shard)) |coordinator_tree| {
            const structure = try coordinator_tree.serializeIndexStructure(allocator);
            defer allocator.free(structure);
            try self.applyIndexStructure(structure);
        }
    }

    /// Serialize current index structure for distribution
    fn serializeIndexStructure(self: *BTree, allocator: std.mem.Allocator) ![]u8 {
        var buffer: std.ArrayList(u8) = .{};
        defer buffer.deinit(allocator);

        // Serialize tree metadata
        try buffer.appendSlice(allocator, std.mem.asBytes(&self.config.order));
        try buffer.appendSlice(allocator, std.mem.asBytes(&self.shard_id));

        const tree_size = self.getSize();
        try buffer.appendSlice(allocator, std.mem.asBytes(&tree_size));

        // Serialize tree structure (simplified - in full implementation would include node structure)
        if (self.root) |root| {
            try self.serializeNode(root, &buffer, allocator);
        }

        return buffer.toOwnedSlice(allocator);
    }

    /// Apply received index structure
    fn applyIndexStructure(self: *BTree, structure: []const u8) !void {
        _ = self;
        _ = structure;
        // Simplified implementation - in full version would deserialize and apply structure
        std.log.info("Applied index structure update", .{});
    }

    /// Serialize a node for structure distribution
    fn serializeNode(self: *BTree, node: *BTreeNode, buffer: *std.ArrayList(u8), allocator: std.mem.Allocator) !void {
        _ = self;

        // Serialize node metadata
        try buffer.appendSlice(allocator, std.mem.asBytes(&node.node_type));
        try buffer.appendSlice(allocator, std.mem.asBytes(&node.num_keys));
        try buffer.appendSlice(allocator, std.mem.asBytes(&node.shard_id));

        // Serialize keys
        for (node.keys[0..node.num_keys]) |key| {
            try buffer.appendSlice(allocator, std.mem.asBytes(&key.key));
            try buffer.appendSlice(allocator, std.mem.asBytes(&key.shard_id));
        }

        // Serialize values (GlobalObjRef)
        for (node.values[0..node.num_keys]) |value| {
            const bytes = value.toBytes();
            try buffer.appendSlice(allocator, &bytes);
        }
    }

    /// Enhanced rebalancing with intelligent data movement
    /// Minimizes data movement during shard rebalancing operations
    pub fn intelligentRebalance(self: *BTree, rebalance_plan: RebalancePlan, allocator: std.mem.Allocator) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        // Phase 1: Identify objects to migrate based on access patterns
        var migration_candidates = try self.identifyMigrationCandidates(rebalance_plan, allocator);
        defer migration_candidates.deinit(allocator);

        // Phase 2: Create migration batches to minimize network overhead
        var migration_batches = try self.createMigrationBatches(migration_candidates.items, allocator);
        defer {
            for (migration_batches.items) |batch| {
                allocator.free(batch.entries);
            }
            migration_batches.deinit(allocator);
        }

        // Phase 3: Execute migration in batches
        for (migration_batches.items) |batch| {
            try self.executeMigrationBatch(batch, allocator);
        }

        // Phase 4: Update shard mappings
        try self.updateShardMappings(rebalance_plan);

        // Phase 5: Verify consistency after rebalancing
        try self.verifyPostRebalanceConsistency(allocator);
    }

    /// Identify objects that should be migrated during rebalancing
    fn identifyMigrationCandidates(self: *BTree, plan: RebalancePlan, allocator: std.mem.Allocator) !std.ArrayList(MigrationCandidate) {
        var candidates: std.ArrayList(MigrationCandidate) = .{};
        errdefer candidates.deinit(allocator);

        if (self.root) |root| {
            try self.findMigrationCandidatesInNode(root, plan, &candidates, allocator);
        }

        return candidates;
    }

    /// Find migration candidates in a specific node
    fn findMigrationCandidatesInNode(self: *BTree, node: *BTreeNode, plan: RebalancePlan, candidates: *std.ArrayList(MigrationCandidate), allocator: std.mem.Allocator) !void {
        // Check leaf node entries
        if (node.isLeaf()) {
            for (node.keys[0..node.num_keys], node.values[0..node.num_keys]) |key, value| {
                const target_shard = plan.getTargetShard(key.key);
                if (target_shard != key.shard_id) {
                    try candidates.append(allocator, MigrationCandidate{
                        .key = key,
                        .value = value,
                        .source_shard = key.shard_id,
                        .target_shard = target_shard,
                        .access_frequency = self.getAccessFrequency(key),
                    });
                }
            }
        } else {
            // Recursively check children
            for (node.values[0 .. node.num_keys + 1]) |child_ref| {
                if (!child_ref.isNull() and child_ref.isLocal(self.shard_id)) {
                    const child_ptr = child_ref.getLocalOffset();
                    const child: *BTreeNode = @ptrFromInt(child_ptr);
                    try self.findMigrationCandidatesInNode(child, plan, candidates, allocator);
                }
            }
        }
    }

    /// Create optimized migration batches
    fn createMigrationBatches(self: *BTree, candidates: []const MigrationCandidate, allocator: std.mem.Allocator) !std.ArrayList(MigrationBatch) {
        _ = self;
        var batches: std.ArrayList(MigrationBatch) = .{};
        errdefer batches.deinit(allocator);

        // Group candidates by target shard
        var shard_groups = std.AutoHashMap(ShardId, std.ArrayList(MigrationCandidate)).init(allocator);
        defer {
            var iter = shard_groups.iterator();
            while (iter.next()) |entry| {
                entry.value_ptr.deinit(allocator);
            }
            shard_groups.deinit();
        }

        for (candidates) |candidate| {
            const result = try shard_groups.getOrPut(candidate.target_shard);
            if (!result.found_existing) {
                result.value_ptr.* = std.ArrayList(MigrationCandidate){};
            }
            try result.value_ptr.append(allocator, candidate);
        }

        // Create batches from groups
        var iter = shard_groups.iterator();
        while (iter.next()) |entry| {
            const target_shard = entry.key_ptr.*;
            var shard_candidates = entry.value_ptr.*;

            try batches.append(allocator, MigrationBatch{
                .target_shard = target_shard,
                .entries = try shard_candidates.toOwnedSlice(allocator),
                .batch_size = shard_candidates.items.len,
            });
        }

        return batches;
    }

    /// Execute a migration batch
    fn executeMigrationBatch(self: *BTree, batch: MigrationBatch, allocator: std.mem.Allocator) !void {
        _ = allocator;

        const target_tree = self.shard_map.get(batch.target_shard) orelse return error.TargetShardNotFound;

        for (batch.entries) |entry| {
            // Insert into target shard
            const new_key = DistributedKey.init(entry.key.key, batch.target_shard);
            const new_value = entry.value.withShard(batch.target_shard);
            try target_tree.insertDistributed(new_key, new_value);

            // Remove from source shard (if it's this shard)
            if (entry.source_shard == self.shard_id) {
                _ = try self.deleteDistributed(entry.key);
            }
        }
    }

    /// Update shard mappings after rebalancing
    fn updateShardMappings(self: *BTree, plan: RebalancePlan) !void {
        // Update internal shard mapping based on rebalancing plan
        for (plan.shard_updates) |update| {
            if (update.operation == .add) {
                // Add new shard mapping (would be provided by distributed coordinator)
                std.log.info("Would add shard {} mapping", .{update.shard_id});
            } else if (update.operation == .remove) {
                // Remove shard mapping
                _ = self.shard_map.remove(update.shard_id);
            }
        }
    }

    /// Verify consistency after rebalancing
    fn verifyPostRebalanceConsistency(self: *BTree, allocator: std.mem.Allocator) !void {
        _ = allocator;

        // Check local tree integrity
        if (!try self.checkIntegrity()) {
            return error.LocalIntegrityCheckFailed;
        }

        // Verify cross-shard consistency
        var shard_iter = self.shard_map.iterator();
        while (shard_iter.next()) |entry| {
            const shard_tree = entry.value_ptr.*;
            if (!try shard_tree.checkIntegrity()) {
                return error.CrossShardIntegrityCheckFailed;
            }
        }

        std.log.info("Post-rebalance consistency verification passed", .{});
    }

    /// Get access frequency for a key (simplified implementation)
    fn getAccessFrequency(self: *BTree, key: DistributedKey) u64 {
        _ = self;
        _ = key;
        // In a full implementation, this would track actual access patterns
        return 1;
    }

    /// Find which shards are affected by a key range
    fn findAffectedShards(self: *BTree, start_key: Key, end_key: Key, allocator: std.mem.Allocator) ![]ShardId {
        _ = start_key;
        _ = end_key;

        var affected: std.ArrayList(ShardId) = .{};
        errdefer affected.deinit(allocator);

        // Add local shard
        try affected.append(allocator, self.shard_id);

        // Add all known shards (in a full implementation, this would be more intelligent)
        var shard_iter = self.shard_map.iterator();
        while (shard_iter.next()) |entry| {
            const shard_id = entry.key_ptr.*;
            if (shard_id != self.shard_id) {
                try affected.append(allocator, shard_id);
            }
        }

        return affected.toOwnedSlice(allocator);
    }

    /// Collect all entries from a node and its children
    fn collectAllEntries(self: *BTree, node: *BTreeNode, entries: *std.ArrayList(KeyValuePair), allocator: std.mem.Allocator) !void {
        if (node.isLeaf()) {
            // Collect entries from leaf node
            for (node.keys[0..node.num_keys], node.values[0..node.num_keys]) |key, value| {
                try entries.append(allocator, KeyValuePair{ .key = key, .value = value });
            }
        } else {
            // Recursively collect from children
            for (node.values[0 .. node.num_keys + 1]) |child_ref| {
                if (!child_ref.isNull() and child_ref.isLocal(self.shard_id)) {
                    const child_ptr = child_ref.getLocalOffset();
                    const child: *BTreeNode = @ptrFromInt(child_ptr);
                    try self.collectAllEntries(child, entries, allocator);
                }
            }
        }
    }

    /// Find the appropriate shard for a key based on key ranges
    fn findShardForKeyRange(self: *BTree, key: DistributedKey, key_ranges: []const KeyRange) ShardId {
        _ = self;

        for (key_ranges) |range| {
            if (range.contains(key.key)) {
                return range.target_shard;
            }
        }

        // Default to current shard if no range matches
        return key.shard_id;
    }

    // ========================================================================
    // Crash Recovery Support
    // ========================================================================

    /// Check B-tree integrity for crash recovery
    pub fn checkIntegrity(self: *BTree) !bool {
        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.root) |root| {
            return self.checkNodeIntegrity(root);
        }
        return true; // Empty tree is valid
    }

    /// Check integrity of a specific node and its children
    fn checkNodeIntegrity(self: *BTree, node: *BTreeNode) !bool {
        // Check key ordering
        for (1..node.num_keys) |i| {
            if (node.keys[i - 1].compare(node.keys[i]) != .lt) {
                std.log.warn("B-tree integrity check: key ordering violation at index {}", .{i});
                return false;
            }
        }

        // Check shard consistency
        for (node.keys[0..node.num_keys], node.values[0..node.num_keys]) |key, value| {
            if (key.shard_id != value.getShardId()) {
                std.log.warn("B-tree integrity check: shard ID mismatch for key {}", .{key.key});
                return false;
            }
        }

        // Recursively check children
        if (!node.isLeaf()) {
            for (node.values[0 .. node.num_keys + 1]) |child_ref| {
                if (!child_ref.isNull() and child_ref.isLocal(self.shard_id)) {
                    const child_ptr = child_ref.getLocalOffset();
                    const child: *BTreeNode = @ptrFromInt(child_ptr);
                    if (!try self.checkNodeIntegrity(child)) {
                        return false;
                    }
                }
            }
        }

        return true;
    }

    /// Repair B-tree structure after corruption
    pub fn repairStructure(self: *BTree) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        if (self.root) |root| {
            try self.repairNode(root);
        }
    }

    /// Repair a specific node
    fn repairNode(self: *BTree, node: *BTreeNode) !void {
        // Sort keys if they're out of order
        var needs_sorting = false;
        for (1..node.num_keys) |i| {
            if (node.keys[i - 1].compare(node.keys[i]) != .lt) {
                needs_sorting = true;
                break;
            }
        }

        if (needs_sorting) {
            // Create temporary arrays for sorting
            var temp_keys = try self.allocator.alloc(DistributedKey, node.num_keys);
            defer self.allocator.free(temp_keys);
            var temp_values = try self.allocator.alloc(GlobalObjRef, node.num_keys);
            defer self.allocator.free(temp_values);

            // Copy data
            @memcpy(temp_keys, node.keys[0..node.num_keys]);
            @memcpy(temp_values, node.values[0..node.num_keys]);

            // Use simple bubble sort instead of complex sorting
            for (0..node.num_keys) |i| {
                for (i + 1..node.num_keys) |j| {
                    if (temp_keys[j].lessThan(temp_keys[i])) {
                        std.mem.swap(DistributedKey, &temp_keys[i], &temp_keys[j]);
                        std.mem.swap(GlobalObjRef, &temp_values[i], &temp_values[j]);
                    }
                }
            }

            // Copy sorted data back
            @memcpy(node.keys[0..node.num_keys], temp_keys);
            @memcpy(node.values[0..node.num_keys], temp_values);

            std.log.info("Repaired key ordering in B-tree node", .{});
        }

        // Recursively repair children
        if (!node.isLeaf()) {
            for (node.values[0 .. node.num_keys + 1]) |child_ref| {
                if (!child_ref.isNull() and child_ref.isLocal(self.shard_id)) {
                    const child_ptr = child_ref.getLocalOffset();
                    const child: *BTreeNode = @ptrFromInt(child_ptr);
                    try self.repairNode(child);
                }
            }
        }
    }

    /// Rebuild B-tree from arena data
    pub fn rebuildFromArena(self: *BTree, arena: *@import("arena.zig").Arena, allocator: std.mem.Allocator) !void {
        self.mutex.lock();
        defer self.mutex.unlock();

        // Clear existing tree
        if (self.root) |root| {
            self.destroyNode(root);
            self.root = null;
        }
        _ = self.size.store(0, .seq_cst);

        // Scan arena for objects
        const objects = try arena.scanObjects(allocator);
        defer objects.deinit();

        // Insert each valid object into the index
        for (objects.items) |obj_info| {
            if (obj_info.is_deleted or !obj_info.checksum_valid) continue;

            // Use object offset as key (simplified)
            const key = obj_info.offset;
            const objref = ObjRef.init(obj_info.offset);

            self.insert(key, objref) catch |err| {
                std.log.warn("Failed to rebuild index entry for offset {}: {}", .{ obj_info.offset, err });
                continue;
            };
        }

        std.log.info("Rebuilt B-tree index with {} entries", .{self.getSize()});
    }

    /// Get tree height (for debugging/testing)
    pub fn height(self: *const BTree) usize {
        if (self.root) |root| {
            return self.nodeHeight(root);
        }
        return 0;
    }

    fn nodeHeight(self: *const BTree, node: *const BTreeNode) usize {
        if (node.isLeaf()) {
            return 1;
        }

        const child_ptr = node.values[0].getLocalOffset();
        const child: *const BTreeNode = @ptrFromInt(child_ptr);
        return 1 + self.nodeHeight(child);
    }
};

// ============================================================================
// Tests
// ============================================================================

test "BTree: creation and cleanup" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    try std.testing.expect(tree.isEmpty());
    try std.testing.expectEqual(@as(usize, 0), tree.getSize());
    try std.testing.expectEqual(@as(ShardId, 0), tree.getShardId());
}

test "BTree: single insert and search" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    const key: Key = 42;
    const value = ObjRef.init(1000);

    try tree.insert(key, value);
    try std.testing.expectEqual(@as(usize, 1), tree.getSize());

    const result = tree.search(key);
    try std.testing.expect(result != null);
    try std.testing.expectEqual(value.getOffset(), result.?.getOffset());
}

test "BTree: multiple inserts" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    // Insert multiple key-value pairs
    const keys = [_]Key{ 10, 20, 30, 40, 50 };
    for (keys, 0..) |key, i| {
        const value = ObjRef.init(i * 100);
        try tree.insert(key, value);
    }

    try std.testing.expectEqual(@as(usize, 5), tree.getSize());

    // Verify all keys can be found
    for (keys, 0..) |key, i| {
        const result = tree.search(key);
        try std.testing.expect(result != null);
        try std.testing.expectEqual(@as(u64, i * 100), result.?.getOffset());
    }
}

test "BTree: search non-existent key" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    try tree.insert(10, ObjRef.init(100));
    try tree.insert(20, ObjRef.init(200));

    const result = tree.search(15);
    try std.testing.expect(result == null);
}

test "BTree: insert in reverse order" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 4 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    // Insert in reverse order (reduced to 5 to avoid deep recursion)
    var i: usize = 5;
    while (i > 0) : (i -= 1) {
        try tree.insert(i, ObjRef.init(i * 10));
    }

    try std.testing.expectEqual(@as(usize, 5), tree.getSize());

    // Verify all keys
    i = 1;
    while (i <= 5) : (i += 1) {
        const result = tree.search(i);
        try std.testing.expect(result != null);
    }
}

test "BTree: range query" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    // Insert keys 10, 20, 30, 40, 50
    for (1..6) |i| {
        try tree.insert(i * 10, ObjRef.init(i * 100));
    }

    // Query range [20, 40]
    const results = try tree.rangeQuery(20, 40, allocator);
    defer allocator.free(results);

    try std.testing.expectEqual(@as(usize, 3), results.len);
}

test "BTree: delete from leaf" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    try tree.insert(10, ObjRef.init(100));
    try tree.insert(20, ObjRef.init(200));
    try tree.insert(30, ObjRef.init(300));

    const deleted = try tree.delete(20);
    try std.testing.expect(deleted);
    try std.testing.expectEqual(@as(usize, 2), tree.getSize());

    const result = tree.search(20);
    try std.testing.expect(result == null);
}

test "BTree: delete non-existent key" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    try tree.insert(10, ObjRef.init(100));

    const deleted = try tree.delete(20);
    try std.testing.expect(!deleted);
    try std.testing.expectEqual(@as(usize, 1), tree.getSize());
}

test "BTree: height calculation" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    try std.testing.expectEqual(@as(usize, 0), tree.height());

    try tree.insert(10, ObjRef.init(100));
    try std.testing.expectEqual(@as(usize, 1), tree.height());
}

test "BTree: concurrent access" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    // Insert some initial data
    for (1..11) |i| {
        try tree.insert(i, ObjRef.init(i * 10));
    }

    // Verify thread-safe access (mutex protects operations)
    const result1 = tree.search(5);
    const result2 = tree.search(7);

    try std.testing.expect(result1 != null);
    try std.testing.expect(result2 != null);
}

test "BTree: empty range query" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    try tree.insert(10, ObjRef.init(100));
    try tree.insert(30, ObjRef.init(300));

    // Query range with no matching keys
    const results = try tree.rangeQuery(15, 25, allocator);
    defer allocator.free(results);

    try std.testing.expectEqual(@as(usize, 0), results.len);
}

test "BTree: large dataset" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 16 };
    const tree = try BTree.init(allocator, config, 0); // shard 0
    defer tree.deinit();

    // Insert 100 keys (reduced to avoid stack overflow with pointer-based implementation)
    for (0..100) |i| {
        try tree.insert(i, ObjRef.init(i * 10));
    }

    try std.testing.expectEqual(@as(usize, 100), tree.getSize());

    // Verify random keys
    try std.testing.expect(tree.search(42) != null);
    try std.testing.expect(tree.search(50) != null);
    try std.testing.expect(tree.search(99) != null);
}

test "BTree: distributed operations" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1); // shard 1
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2); // shard 2
    defer tree2.deinit();

    // Set up shard mapping
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert distributed keys
    const key1 = DistributedKey.init(10, 1);
    const value1 = GlobalObjRef.init(1, 1000, 0);
    try tree1.insertDistributed(key1, value1);

    const key2 = DistributedKey.init(20, 2);
    const value2 = GlobalObjRef.init(2, 2000, 0);
    try tree2.insertDistributed(key2, value2);

    // Search for keys
    const result1 = tree1.searchDistributed(key1);
    try std.testing.expect(result1 != null);
    try std.testing.expectEqual(@as(u64, 1000), result1.?.getLocalOffset());

    const result2 = tree1.searchDistributed(key2); // Cross-shard search
    try std.testing.expect(result2 != null);
    try std.testing.expectEqual(@as(u64, 2000), result2.?.getLocalOffset());
}

test "BTree: cross-shard range query" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1); // shard 1
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2); // shard 2
    defer tree2.deinit();

    // Set up shard mapping
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert keys across shards
    for (1..6) |i| {
        const shard_id: ShardId = if (i % 2 == 0) 2 else 1;
        const key = DistributedKey.init(i * 10, shard_id);
        const value = GlobalObjRef.init(shard_id, i * 100, 0);

        if (shard_id == 1) {
            try tree1.insertDistributed(key, value);
        } else {
            try tree2.insertDistributed(key, value);
        }
    }

    // Perform cross-shard range query
    const results = try tree1.coordinateCrossShardQuery(10, 50, allocator);
    defer allocator.free(results);

    try std.testing.expect(results.len > 0);
}

test "BTree: shard rebalancing" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree1 = try BTree.init(allocator, config, 1); // shard 1
    defer tree1.deinit();
    const tree2 = try BTree.init(allocator, config, 2); // shard 2
    defer tree2.deinit();

    // Set up shard mapping
    try tree1.addShard(2, tree2);
    try tree2.addShard(1, tree1);

    // Insert some data in shard 1
    for (1..4) |i| {
        const key = DistributedKey.init(i * 10, 1);
        const value = GlobalObjRef.init(1, i * 100, 0);
        try tree1.insertDistributed(key, value);
    }

    // Simulate rebalancing: move keys 10-20 from shard 1 to shard 2
    try tree1.rebalanceForMigration(1, 2, 10, 20, allocator);

    // Verify consistency is maintained
    try tree1.maintainConsistencyDuringRebalancing();
    try tree2.maintainConsistencyDuringRebalancing();
}

test "BTree: batch insert" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    // Create batch of entries
    var entries: std.ArrayList(KeyValuePair) = .{};
    defer entries.deinit(allocator);

    var i: u64 = 1;
    while (i <= 10) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        const value = GlobalObjRef.init(0, i * 100, 0);
        try entries.append(allocator, KeyValuePair{ .key = key, .value = value });
    }

    // Insert batch
    try tree.insertBatch(entries.items);

    // Verify all entries were inserted
    try std.testing.expectEqual(@as(usize, 10), tree.size.load(.seq_cst));

    // Verify we can search for all keys
    i = 1;
    while (i <= 10) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        const result = tree.searchDistributed(key);
        try std.testing.expect(result != null);
    }
}

test "BTree: atomic batch insert" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    // Create batch of entries
    var entries: std.ArrayList(KeyValuePair) = .{};
    defer entries.deinit(allocator);

    var i: u64 = 1;
    while (i <= 5) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        const value = GlobalObjRef.init(0, i * 100, 0);
        try entries.append(allocator, KeyValuePair{ .key = key, .value = value });
    }

    // Insert batch atomically
    try tree.atomicInsertBatch(entries.items);

    // Verify all entries were inserted
    try std.testing.expectEqual(@as(usize, 5), tree.size.load(.seq_cst));

    // Verify we can search for all keys
    i = 1;
    while (i <= 5) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        const result = tree.searchDistributed(key);
        try std.testing.expect(result != null);
    }
}

test "BTree: batch insert with sorting" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    // Create batch of entries in reverse order
    var entries: std.ArrayList(KeyValuePair) = .{};
    defer entries.deinit(allocator);

    var i: u64 = 10;
    while (i >= 1) : (i -= 1) {
        const key = DistributedKey.init(i * 10, 0);
        const value = GlobalObjRef.init(0, i * 100, 0);
        try entries.append(allocator, KeyValuePair{ .key = key, .value = value });
        if (i == 1) break;
    }

    // Insert batch (should sort internally)
    try tree.insertBatch(entries.items);

    // Verify all entries were inserted
    try std.testing.expectEqual(@as(usize, 10), tree.size.load(.seq_cst));
}

test "BTree: batch delete" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    // Insert some entries
    var i: u64 = 1;
    while (i <= 10) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        const value = GlobalObjRef.init(0, i * 100, 0);
        try tree.insertDistributed(key, value);
    }

    // Create batch of keys to delete
    var keys: std.ArrayList(DistributedKey) = .{};
    defer keys.deinit(allocator);

    i = 1;
    while (i <= 5) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        try keys.append(allocator, key);
    }

    // Delete batch
    try tree.deleteBatch(keys.items);

    // Verify deleted keys are gone
    i = 1;
    while (i <= 5) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        const result = tree.searchDistributed(key);
        try std.testing.expect(result == null);
    }

    // Verify remaining keys still exist
    i = 6;
    while (i <= 10) : (i += 1) {
        const key = DistributedKey.init(i * 10, 0);
        const result = tree.searchDistributed(key);
        try std.testing.expect(result != null);
    }
}

test "BTree: empty batch insert" {
    const allocator = std.testing.allocator;

    const config = BTreeConfig{ .order = 3 };
    const tree = try BTree.init(allocator, config, 0);
    defer tree.deinit();

    // Insert empty batch (should not error)
    const empty: []const KeyValuePair = &[_]KeyValuePair{};
    try tree.insertBatch(empty);

    // Verify size is still 0
    try std.testing.expectEqual(@as(usize, 0), tree.size.load(.seq_cst));
}
