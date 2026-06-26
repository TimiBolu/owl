//! WalIntentRing prototype for WAL ingress.
//!
//! This module implements a narrow ring-decomposed ingress path inspired by
//! `ringmpsc-main`. It is intentionally conservative:
//! - one ring per producer
//! - one synchronous writer-owned flush loop
//! - existing WAL framing and recovery behavior retained
//! - success published only after durable completion

const std = @import("std");
const Allocator = std.mem.Allocator;
const File = std.fs.File;
const ObjRef = @import("objref.zig").ObjRef;

const WALHeader = @import("wal.zig").WALHeader;
const WALStats = @import("wal.zig").WALStats;
const GroupCommitConfig = @import("wal.zig").GroupCommitConfig;
const MAX_WAL_SIZE = @import("wal.zig").MAX_WAL_SIZE;
const WAL_ALIGNMENT = @import("wal.zig").WAL_ALIGNMENT;

pub const LSN = @import("wal.zig").LSN;
pub const EntryType = @import("wal.zig").EntryType;
pub const WALEntryHeader = @import("wal.zig").WALEntryHeader;
pub const WALEntry = @import("wal.zig").WALEntry;

pub const DurabilityClass = enum(u8) {
    buffered = 1,
    immediate = 2,
};

pub const CompletionStatus = enum(u8) {
    empty = 0,
    pending = 1,
    durable = 2,
    failed = 3,
};

pub const WalIntentRingConfig = struct {
    producer_count: u32 = 4,
    ring_capacity: usize = 1024,
    completion_capacity: usize = 1024,
    max_batch_entries: usize = 256,
    max_batch_bytes: usize = 1024 * 1024,
    per_ring_batch_cap: usize = 32,
    spawn_writer_thread: bool = false,
    writer_idle_sleep_ns: u64 = 50_000,
};

pub const SubmissionToken = struct {
    producer_id: u32,
    completion_slot: u32,
    sequence_local: u64,
};

pub const CompletionSnapshot = struct {
    sequence_local: u64,
    final_lsn: LSN,
    final_offset: u64,
    error_code: i32,
    status: CompletionStatus,

    pub fn isTerminal(self: CompletionSnapshot) bool {
        return self.status == .durable or self.status == .failed;
    }
};

const CompletionRecord = struct {
    sequence_local: u64,
    final_lsn: LSN,
    final_offset: u64,
    error_code: i32,
    status: std.atomic.Value(u8),

    fn init() CompletionRecord {
        return .{
            .sequence_local = 0,
            .final_lsn = 0,
            .final_offset = 0,
            .error_code = 0,
            .status = std.atomic.Value(u8).init(@intFromEnum(CompletionStatus.empty)),
        };
    }

    fn markPending(self: *CompletionRecord, sequence_local: u64) void {
        self.sequence_local = sequence_local;
        self.final_lsn = 0;
        self.final_offset = 0;
        self.error_code = 0;
        self.status.store(@intFromEnum(CompletionStatus.pending), .release);
    }

    fn publishSuccess(self: *CompletionRecord, sequence_local: u64, final_lsn: LSN, final_offset: u64) void {
        self.sequence_local = sequence_local;
        self.final_lsn = final_lsn;
        self.final_offset = final_offset;
        self.error_code = 0;
        self.status.store(@intFromEnum(CompletionStatus.durable), .release);
    }

    fn publishFailure(self: *CompletionRecord, sequence_local: u64, error_code: i32) void {
        self.sequence_local = sequence_local;
        self.final_lsn = 0;
        self.final_offset = 0;
        self.error_code = error_code;
        self.status.store(@intFromEnum(CompletionStatus.failed), .release);
    }

    fn snapshot(self: *const CompletionRecord) CompletionSnapshot {
        const status = @as(CompletionStatus, @enumFromInt(self.status.load(.acquire)));
        return .{
            .sequence_local = self.sequence_local,
            .final_lsn = self.final_lsn,
            .final_offset = self.final_offset,
            .error_code = self.error_code,
            .status = status,
        };
    }
};

const WalIntent = struct {
    sequence_local: u64,
    completion_slot: u32,
    durability_class: DurabilityClass,
    entry: WALEntry,
};

const RingSlot = struct {
    intent: ?WalIntent = null,
};

const ProducerRing = struct {
    producer_id: u32,
    slots: []RingSlot,
    completion_slots: []CompletionRecord,
    tail: std.atomic.Value(u64),
    head: std.atomic.Value(u64),
    cached_head: u64,
    cached_tail: u64,
    next_sequence: u64,
    next_completion_slot: u32,
    dropped_intents: std.atomic.Value(u64),

    fn init(allocator: Allocator, producer_id: u32, ring_capacity: usize, completion_capacity: usize) !ProducerRing {
        const slots = try allocator.alloc(RingSlot, ring_capacity);
        errdefer allocator.free(slots);
        for (slots) |*slot| slot.* = .{};

        const completion_slots = try allocator.alloc(CompletionRecord, completion_capacity);
        errdefer allocator.free(completion_slots);
        for (completion_slots) |*slot| slot.* = CompletionRecord.init();

        return .{
            .producer_id = producer_id,
            .slots = slots,
            .completion_slots = completion_slots,
            .tail = std.atomic.Value(u64).init(0),
            .head = std.atomic.Value(u64).init(0),
            .cached_head = 0,
            .cached_tail = 0,
            .next_sequence = 1,
            .next_completion_slot = 0,
            .dropped_intents = std.atomic.Value(u64).init(0),
        };
    }

    fn deinit(self: *ProducerRing, allocator: Allocator) void {
        const head = self.head.load(.acquire);
        const tail = self.tail.load(.acquire);
        var cursor = head;
        while (cursor < tail) : (cursor += 1) {
            const idx = cursor & (self.slots.len - 1);
            if (self.slots[idx].intent) |*intent| {
                intent.entry.deinit(allocator);
                self.slots[idx].intent = null;
            }
        }

        allocator.free(self.slots);
        allocator.free(self.completion_slots);
    }

    fn reserveCompletionSlot(self: *ProducerRing) !u32 {
        const idx = self.next_completion_slot;
        const slot = &self.completion_slots[idx];
        const status = @as(CompletionStatus, @enumFromInt(slot.status.load(.acquire)));
        if (status == .pending) return error.CompletionSlotsExhausted;
        self.next_completion_slot = @intCast((idx + 1) % self.completion_slots.len);
        return idx;
    }

    fn enqueue(self: *ProducerRing, entry: WALEntry, durability_class: DurabilityClass) !SubmissionToken {
        const tail = self.tail.load(.monotonic);
        if (tail - self.cached_head >= self.slots.len) {
            self.cached_head = self.head.load(.acquire);
            if (tail - self.cached_head >= self.slots.len) {
                _ = self.dropped_intents.fetchAdd(1, .monotonic);
                return error.RingFull;
            }
        }

        const completion_slot = try self.reserveCompletionSlot();
        const sequence_local = self.next_sequence;
        self.next_sequence += 1;
        self.completion_slots[completion_slot].markPending(sequence_local);

        const idx = tail & (self.slots.len - 1);
        self.slots[idx].intent = .{
            .sequence_local = sequence_local,
            .completion_slot = completion_slot,
            .durability_class = durability_class,
            .entry = entry,
        };
        self.tail.store(tail + 1, .release);

        return .{
            .producer_id = self.producer_id,
            .completion_slot = completion_slot,
            .sequence_local = sequence_local,
        };
    }

    fn available(self: *ProducerRing) u64 {
        const head = self.head.load(.acquire);
        const tail = self.tail.load(.acquire);
        return tail - head;
    }
};

const WalBatchEntry = struct {
    producer_id: u32,
    completion_slot: u32,
    sequence_local: u64,
    durability_class: DurabilityClass,
    entry: WALEntry,
    final_lsn: LSN = 0,
    final_offset: u64 = 0,
};

pub const ProducerHandle = struct {
    wal: *WalIntentRingWAL,
    producer_id: u32,

    pub fn appendBuffered(self: *ProducerHandle, entry: WALEntry) !SubmissionToken {
        return self.wal.submitOwned(self.producer_id, entry, .buffered);
    }

    pub fn appendImmediate(self: *ProducerHandle, entry: WALEntry) !SubmissionToken {
        return self.wal.submitOwned(self.producer_id, entry, .immediate);
    }

    pub fn getCompletion(self: *ProducerHandle, token: SubmissionToken) CompletionSnapshot {
        return self.wal.getCompletion(token);
    }
};

pub const WalIntentRingWAL = struct {
    allocator: Allocator,
    base_path: []const u8,
    current_file: ?File,
    current_lsn: std.atomic.Value(LSN),
    current_offset: std.atomic.Value(u64),
    entry_count: std.atomic.Value(u64),
    pending_count: std.atomic.Value(usize),
    stats: WALStats,
    group_commit_config: GroupCommitConfig,
    ring_config: WalIntentRingConfig,
    producer_rings: []ProducerRing,
    rr_cursor: u32,
    last_flush_time: std.atomic.Value(i64),
    inflight_count: std.atomic.Value(usize),
    flush_requested: std.atomic.Value(bool),
    shutdown_requested: std.atomic.Value(bool),
    writer_thread: ?std.Thread,
    writer_mutex: std.Thread.Mutex,

    const Self = @This();

    pub fn init(allocator: Allocator, base_path: []const u8) !Self {
        return initWithConfig(allocator, base_path, GroupCommitConfig{}, WalIntentRingConfig{});
    }

    pub fn initWithConfig(allocator: Allocator, base_path: []const u8, group_commit_config: GroupCommitConfig, ring_config: WalIntentRingConfig) !Self {
        if (!std.math.isPowerOfTwo(ring_config.ring_capacity)) return error.RingCapacityMustBePowerOfTwo;
        if (ring_config.producer_count == 0) return error.InvalidProducerCount;
        if (ring_config.completion_capacity == 0) return error.InvalidCompletionCapacity;

        std.fs.cwd().makeDir(base_path) catch |err| switch (err) {
            error.PathAlreadyExists => {},
            else => return err,
        };

        var wal = Self{
            .allocator = allocator,
            .base_path = try allocator.dupe(u8, base_path),
            .current_file = null,
            .current_lsn = std.atomic.Value(LSN).init(1),
            .current_offset = std.atomic.Value(u64).init(0),
            .entry_count = std.atomic.Value(u64).init(0),
            .pending_count = std.atomic.Value(usize).init(0),
            .stats = WALStats.init(),
            .group_commit_config = group_commit_config,
            .ring_config = ring_config,
            .producer_rings = try allocator.alloc(ProducerRing, ring_config.producer_count),
            .rr_cursor = 0,
            .last_flush_time = std.atomic.Value(i64).init(std.time.milliTimestamp()),
            .inflight_count = std.atomic.Value(usize).init(0),
            .flush_requested = std.atomic.Value(bool).init(false),
            .shutdown_requested = std.atomic.Value(bool).init(false),
            .writer_thread = null,
            .writer_mutex = .{},
        };
        errdefer {
            allocator.free(wal.base_path);
            allocator.free(wal.producer_rings);
        }

        for (wal.producer_rings, 0..) |*ring, producer_id| {
            ring.* = try ProducerRing.init(allocator, @intCast(producer_id), ring_config.ring_capacity, ring_config.completion_capacity);
        }
        errdefer {
            for (wal.producer_rings) |*ring| ring.deinit(allocator);
        }

        try wal.recover();
        try wal.openCurrentFile();

        return wal;
    }

    pub fn startWriterThread(self: *Self) !void {
        if (self.writer_thread != null) return;
        self.shutdown_requested.store(false, .release);
        self.flush_requested.store(false, .release);
        self.writer_thread = try std.Thread.spawn(.{}, writerThreadMain, .{self});
    }

    pub fn deinit(self: *Self) void {
        if (self.writer_thread != null) {
            self.flush_requested.store(true, .release);
            self.shutdown_requested.store(true, .release);
            if (self.writer_thread) |thread| thread.join();
        } else {
            self.flush() catch |err| {
                std.log.err("Failed to flush WalIntentRingWAL during deinit: {}", .{err});
            };
        }

        for (self.producer_rings) |*ring| ring.deinit(self.allocator);
        self.allocator.free(self.producer_rings);

        if (self.current_file) |file| file.close();
        self.allocator.free(self.base_path);
    }

    pub fn registerProducer(self: *Self, producer_id: u32) !ProducerHandle {
        if (producer_id >= self.producer_rings.len) return error.UnknownProducer;
        return .{
            .wal = self,
            .producer_id = producer_id,
        };
    }

    pub fn nextLSN(self: *Self) LSN {
        return self.current_lsn.fetchAdd(1, .seq_cst);
    }

    pub fn getCurrentLSN(self: *Self) LSN {
        return self.current_lsn.load(.seq_cst);
    }

    pub fn append(self: *Self, entry: *const WALEntry) !void {
        var producer = try self.registerProducer(0);
        const owned = try WALEntry.init(self.allocator, 0, entry.header.entry_type, ObjRef.init(entry.header.objref), entry.data);
        const token = try producer.appendImmediate(owned);
        if (self.ring_config.spawn_writer_thread) {
            self.flush_requested.store(true, .release);
        } else {
            try self.flush();
        }
        const completion = try self.waitForCompletion(token, 5_000);
        if (completion.status == .failed) return error.WALAppendFailed;
    }

    pub fn atomicAppend(self: *Self, entry: *const WALEntry) !void {
        return self.append(entry);
    }

    pub fn atomicAppendWithSync(self: *Self, entry: *const WALEntry) !void {
        return self.append(entry);
    }

    pub fn appendBuffered(self: *Self, entry: WALEntry) !void {
        _ = try self.submitOwned(0, entry, .buffered);
        if (self.shouldFlush()) {
            if (self.ring_config.spawn_writer_thread) {
                self.flush_requested.store(true, .release);
            } else {
                try self.flush();
            }
        }
    }

    pub fn submitOwned(self: *Self, producer_id: u32, entry: WALEntry, durability_class: DurabilityClass) !SubmissionToken {
        if (producer_id >= self.producer_rings.len) return error.UnknownProducer;
        const token = try self.producer_rings[producer_id].enqueue(entry, durability_class);
        _ = self.pending_count.fetchAdd(1, .monotonic);
        return token;
    }

    pub fn flush(self: *Self) !void {
        self.writer_mutex.lock();
        defer self.writer_mutex.unlock();
        return self.flushUnlocked();
    }

    fn flushUnlocked(self: *Self) !void {
        var batch = std.ArrayList(WalBatchEntry){};
        defer {
            for (batch.items) |*batch_entry| batch_entry.entry.deinit(self.allocator);
            batch.deinit(self.allocator);
        }

        try self.drainIntoBatch(&batch);
        if (batch.items.len == 0) return;
        _ = self.inflight_count.fetchAdd(batch.items.len, .acq_rel);

        self.writeBatch(batch.items) catch |err| {
            self.publishBatchFailure(batch.items, @as(i32, @intCast(@intFromError(err))));
            return err;
        };
        _ = self.last_flush_time.store(std.time.milliTimestamp(), .release);
    }

    pub fn getCompletion(self: *Self, token: SubmissionToken) CompletionSnapshot {
        if (token.producer_id >= self.producer_rings.len) {
            return .{
                .sequence_local = token.sequence_local,
                .final_lsn = 0,
                .final_offset = 0,
                .error_code = -1,
                .status = .failed,
            };
        }

        const ring = &self.producer_rings[token.producer_id];
        return ring.completion_slots[token.completion_slot].snapshot();
    }

    pub fn checkpoint(self: *Self) !LSN {
        var producer = try self.registerProducer(0);
        const entry = try WALEntry.init(self.allocator, 0, .checkpoint, ObjRef.init(0), "CHECKPOINT");
        const token = try producer.appendImmediate(entry);
        if (self.ring_config.spawn_writer_thread) {
            self.flush_requested.store(true, .release);
        } else {
            try self.flush();
        }
        const completion = try self.waitForCompletion(token, 5_000);
        if (completion.status == .failed) return error.WALAppendFailed;
        self.stats.last_checkpoint = completion.final_lsn;
        return completion.final_lsn;
    }

    pub fn getStats(self: *const Self) WALStats {
        var stats = self.stats;
        stats.entry_count = self.entry_count.load(.monotonic);
        return stats;
    }

    pub fn waitForCompletion(self: *Self, token: SubmissionToken, timeout_ms: u64) !CompletionSnapshot {
        const start = std.time.milliTimestamp();
        while (true) {
            const completion = self.getCompletion(token);
            if (completion.isTerminal()) return completion;
            if (@as(u64, @intCast(std.time.milliTimestamp() - start)) >= timeout_ms) {
                return error.CompletionTimeout;
            }
            if (!self.ring_config.spawn_writer_thread) {
                try self.flush();
            } else {
                self.flush_requested.store(true, .release);
                std.Thread.sleep(self.ring_config.writer_idle_sleep_ns);
            }
        }
    }

    pub fn waitForIdle(self: *Self, timeout_ms: u64) !void {
        const start = std.time.milliTimestamp();
        while (true) {
            if (self.pending_count.load(.acquire) == 0 and self.inflight_count.load(.acquire) == 0) return;
            if (@as(u64, @intCast(std.time.milliTimestamp() - start)) >= timeout_ms) return error.IdleWaitTimeout;
            if (!self.ring_config.spawn_writer_thread) {
                try self.flush();
            } else {
                self.flush_requested.store(true, .release);
                std.Thread.sleep(self.ring_config.writer_idle_sleep_ns);
            }
        }
    }

    fn shouldFlush(self: *Self) bool {
        const count = self.pending_count.load(.monotonic);
        const now = std.time.milliTimestamp();
        const last_flush = self.last_flush_time.load(.monotonic);

        return count >= self.group_commit_config.max_entries or
            (now - last_flush) >= self.group_commit_config.max_wait_ms;
    }

    fn drainIntoBatch(self: *Self, batch: *std.ArrayList(WalBatchEntry)) !void {
        var batch_bytes: usize = 0;
        var first = true;
        const producer_count = self.producer_rings.len;
        const start_cursor = self.rr_cursor;

        var visited: usize = 0;
        while (visited < producer_count) : (visited += 1) {
            const ring_index = @as(usize, @intCast((start_cursor + @as(u32, @intCast(visited))) % @as(u32, @intCast(producer_count))));
            var ring = &self.producer_rings[ring_index];

            var drained_for_ring: usize = 0;
            while (drained_for_ring < self.ring_config.per_ring_batch_cap) : (drained_for_ring += 1) {
                const head = ring.head.load(.acquire);
                if (head == ring.cached_tail) {
                    ring.cached_tail = ring.tail.load(.acquire);
                    if (head == ring.cached_tail) break;
                }

                const slot_index = head & (ring.slots.len - 1);
                var intent = ring.slots[slot_index].intent orelse break;

                const entry_size = intent.entry.alignedSize();
                if (batch.items.len >= self.ring_config.max_batch_entries) return self.finishDrain(batch, producer_count);
                if (!first and batch_bytes + entry_size > self.ring_config.max_batch_bytes) return self.finishDrain(batch, producer_count);

                ring.slots[slot_index].intent = null;
                ring.head.store(head + 1, .release);
            _ = self.pending_count.fetchSub(1, .monotonic);

                try batch.append(self.allocator, .{
                    .producer_id = ring.producer_id,
                    .completion_slot = intent.completion_slot,
                    .sequence_local = intent.sequence_local,
                    .durability_class = intent.durability_class,
                    .entry = intent.entry,
                });
                batch_bytes += entry_size;
                first = false;
            }
        }

        self.rr_cursor = (start_cursor + 1) % @as(u32, @intCast(producer_count));
    }

    fn finishDrain(self: *Self, batch: *std.ArrayList(WalBatchEntry), producer_count: usize) void {
        _ = batch;
        self.rr_cursor = (self.rr_cursor + 1) % @as(u32, @intCast(producer_count));
    }

    fn writeBatch(self: *Self, batch: []WalBatchEntry) !void {
        var total_size: u64 = 0;
        for (batch) |*batch_entry| total_size += batch_entry.entry.alignedSize();

        const current_offset = self.current_offset.load(.acquire);
        if (current_offset + total_size > MAX_WAL_SIZE) {
            try self.rotateFile();
        }

        const file = self.current_file orelse return error.WALNotOpen;

        var offset = self.current_offset.load(.acquire);
        for (batch) |*batch_entry| {
            batch_entry.final_lsn = self.current_lsn.fetchAdd(1, .seq_cst);
            batch_entry.final_offset = offset;
            batch_entry.entry.header.lsn = batch_entry.final_lsn;
            batch_entry.entry.header.updateChecksum(batch_entry.entry.data);

            try file.seekTo(offset);
            try file.writeAll(std.mem.asBytes(&batch_entry.entry.header));
            try file.writeAll(batch_entry.entry.data);

            const padding_size = batch_entry.entry.alignedSize() - batch_entry.entry.totalSize();
            if (padding_size > 0) {
                const padding = [_]u8{0} ** WAL_ALIGNMENT;
                try file.writeAll(padding[0..padding_size]);
            }

            offset += batch_entry.entry.alignedSize();
        }

        try file.sync();
        self.current_offset.store(offset, .release);

        for (batch) |*batch_entry| {
            _ = self.entry_count.fetchAdd(1, .monotonic);
            self.stats.last_lsn = batch_entry.final_lsn;
            const ring = &self.producer_rings[batch_entry.producer_id];
            ring.completion_slots[batch_entry.completion_slot].publishSuccess(
                batch_entry.sequence_local,
                batch_entry.final_lsn,
                batch_entry.final_offset,
            );
        }
        _ = self.inflight_count.fetchSub(batch.len, .acq_rel);
    }

    fn publishBatchFailure(self: *Self, batch: []WalBatchEntry, error_code: i32) void {
        for (batch) |*batch_entry| {
            const ring = &self.producer_rings[batch_entry.producer_id];
            ring.completion_slots[batch_entry.completion_slot].publishFailure(
                batch_entry.sequence_local,
                error_code,
            );
        }
        _ = self.inflight_count.fetchSub(batch.len, .acq_rel);
    }

    fn recover(self: *Self) !void {
        const RegularWAL = @import("wal.zig").WAL;

        var temp_wal = try RegularWAL.init(self.allocator, self.base_path);
        defer temp_wal.deinit();

        const stats = temp_wal.getStats();
        self.current_lsn.store(temp_wal.getCurrentLSN(), .release);
        self.stats = stats;
        self.entry_count.store(stats.entry_count, .release);
    }

    fn openCurrentFile(self: *Self) !void {
        const filename = try std.fmt.allocPrint(self.allocator, "wal_{:08}.wal", .{self.getCurrentLSN()});
        defer self.allocator.free(filename);

        const file_path = try std.fs.path.join(self.allocator, &[_][]const u8{ self.base_path, filename });
        defer self.allocator.free(file_path);

        self.current_file = try std.fs.cwd().createFile(file_path, .{ .truncate = false });

        const stat = try self.current_file.?.stat();
        self.current_offset.store(stat.size, .release);

        if (stat.size == 0) {
            var header = WALHeader.init();
            header.updateChecksum();
            try self.current_file.?.writeAll(std.mem.asBytes(&header));
            self.current_offset.store(@sizeOf(WALHeader), .release);
        }

        try self.current_file.?.seekTo(self.current_offset.load(.acquire));
    }

    fn rotateFile(self: *Self) !void {
        if (self.current_file) |file| {
            file.close();
            self.current_file = null;
        }

        try self.openCurrentFile();
        self.stats.file_count += 1;
    }

    fn writerThreadMain(self: *Self) void {
        while (true) {
            const shutdown = self.shutdown_requested.load(.acquire);
            const pending = self.pending_count.load(.acquire);
            const flush_requested = self.flush_requested.swap(false, .acq_rel);

            if (pending > 0 and (flush_requested or self.shouldFlush() or shutdown)) {
                while (self.pending_count.load(.acquire) > 0) {
                    self.flush() catch |err| {
                        std.log.err("WalIntentRingWAL writer thread flush failed: {}", .{err});
                        break;
                    };
                    if (!shutdown and !flush_requested and !self.shouldFlush()) break;
                }
            }

            if (shutdown and self.pending_count.load(.acquire) == 0 and self.inflight_count.load(.acquire) == 0) break;

            if (pending == 0 and !flush_requested) {
                std.Thread.sleep(self.ring_config.writer_idle_sleep_ns);
            }
        }
    }
};

test "WalIntentRingWAL: producer submit and flush publishes durable completion" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_intent_ring_basic";

    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WalIntentRingWAL.initWithConfig(
        allocator,
        test_dir,
        .{},
        .{ .producer_count = 2, .ring_capacity = 8, .completion_capacity = 8 },
    );
    defer wal.deinit();

    var producer = try wal.registerProducer(1);
    const entry = try WALEntry.init(allocator, 0, .insert, ObjRef.init(4096), "hello ring wal");
    const token = try producer.appendImmediate(entry);

    try wal.flush();

    const completion = producer.getCompletion(token);
    try std.testing.expectEqual(CompletionStatus.durable, completion.status);
    try std.testing.expect(completion.final_lsn >= 1);
    try std.testing.expect(completion.final_offset > 0);
    try std.testing.expectEqual(@as(u64, 1), wal.getStats().entry_count);
}

test "WalIntentRingWAL: per-producer local sequence stays ordered" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_intent_ring_sequence";

    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WalIntentRingWAL.initWithConfig(
        allocator,
        test_dir,
        .{},
        .{ .producer_count = 2, .ring_capacity = 8, .completion_capacity = 8 },
    );
    defer wal.deinit();

    var producer = try wal.registerProducer(0);

    const entry1 = try WALEntry.init(allocator, 0, .insert, ObjRef.init(64), "one");
    const entry2 = try WALEntry.init(allocator, 0, .update, ObjRef.init(128), "two");

    const t1 = try producer.appendBuffered(entry1);
    const t2 = try producer.appendBuffered(entry2);

    try std.testing.expectEqual(@as(u64, 1), t1.sequence_local);
    try std.testing.expectEqual(@as(u64, 2), t2.sequence_local);

    try wal.flush();

    const c1 = producer.getCompletion(t1);
    const c2 = producer.getCompletion(t2);

    try std.testing.expectEqual(CompletionStatus.durable, c1.status);
    try std.testing.expectEqual(CompletionStatus.durable, c2.status);
    try std.testing.expect(c2.final_lsn > c1.final_lsn);
}

test "WalIntentRingWAL: ring full returns error until drained" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_intent_ring_full";

    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WalIntentRingWAL.initWithConfig(
        allocator,
        test_dir,
        .{ .max_entries = 1000, .max_wait_ms = 10_000 },
        .{ .producer_count = 1, .ring_capacity = 2, .completion_capacity = 4, .max_batch_entries = 4 },
    );
    defer wal.deinit();

    var producer = try wal.registerProducer(0);

    const entry1 = try WALEntry.init(allocator, 0, .insert, ObjRef.init(64), "one");
    const entry2 = try WALEntry.init(allocator, 0, .insert, ObjRef.init(128), "two");
    var entry3 = try WALEntry.init(allocator, 0, .insert, ObjRef.init(192), "three");

    _ = try producer.appendBuffered(entry1);
    _ = try producer.appendBuffered(entry2);
    try std.testing.expectError(error.RingFull, producer.appendBuffered(entry3));
    entry3.deinit(allocator);

    try wal.flush();

    const entry4 = try WALEntry.init(allocator, 0, .insert, ObjRef.init(256), "four");
    _ = try producer.appendBuffered(entry4);
}

test "WalIntentRingWAL: background writer thread drains buffered entries" {
    const allocator = std.testing.allocator;
    const test_dir = "test_wal_intent_ring_background";

    std.fs.cwd().deleteTree(test_dir) catch {};
    defer std.fs.cwd().deleteTree(test_dir) catch {};

    var wal = try WalIntentRingWAL.initWithConfig(
        allocator,
        test_dir,
        .{ .max_entries = 2, .max_wait_ms = 1 },
        .{
            .producer_count = 2,
            .ring_capacity = 16,
            .completion_capacity = 16,
            .spawn_writer_thread = true,
            .writer_idle_sleep_ns = 10_000,
        },
    );
    defer wal.deinit();
    try wal.startWriterThread();

    var producer = try wal.registerProducer(1);
    const entry1 = try WALEntry.init(allocator, 0, .insert, ObjRef.init(1024), "bg-one");
    const entry2 = try WALEntry.init(allocator, 0, .insert, ObjRef.init(2048), "bg-two");

    const t1 = try producer.appendBuffered(entry1);
    const t2 = try producer.appendBuffered(entry2);

    _ = try wal.waitForCompletion(t1, 5_000);
    _ = try wal.waitForCompletion(t2, 5_000);
    try wal.waitForIdle(5_000);

    try std.testing.expectEqual(CompletionStatus.durable, producer.getCompletion(t1).status);
    try std.testing.expectEqual(CompletionStatus.durable, producer.getCompletion(t2).status);
    try std.testing.expectEqual(@as(u64, 2), wal.getStats().entry_count);
}
