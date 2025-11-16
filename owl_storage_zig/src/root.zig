// Root module that exports all public APIs

pub const ObjRef = @import("objref.zig").ObjRef;
pub const ObjectHeader = @import("object_header.zig").ObjectHeader;
pub const Object = @import("object.zig").Object;
pub const Arena = @import("arena.zig").Arena;
pub const ArenaStats = @import("arena.zig").ArenaStats;
pub const CoLocationMetrics = @import("arena.zig").CoLocationMetrics;
pub const PAGE_SIZE = @import("arena.zig").PAGE_SIZE;
pub const SLOT_SIZE = @import("arena.zig").SLOT_SIZE;

// WAL exports
pub const WAL = @import("wal.zig").WAL;
pub const WALEntry = @import("wal.zig").WALEntry;
pub const LSN = @import("wal.zig").LSN;
pub const EntryType = @import("wal.zig").EntryType;
pub const WALStats = @import("wal.zig").WALStats;

// Transaction exports
pub const Transaction = @import("transaction.zig").Transaction;
pub const TransactionId = @import("transaction.zig").TransactionId;
pub const TransactionState = @import("transaction.zig").TransactionState;
pub const TransactionManager = @import("transaction.zig").TransactionManager;
pub const TransactionConfig = @import("transaction.zig").TransactionConfig;
pub const IsolationLevel = @import("transaction.zig").IsolationLevel;
pub const TransactionStats = @import("transaction.zig").TransactionStats;

// Cache exports
pub const LRUCache = @import("cache.zig").LRUCache;
pub const CacheConfig = @import("cache.zig").CacheConfig;
pub const CacheStats = @import("cache.zig").CacheStats;

// Batch Swizzle exports
pub const BatchSwizzler = @import("batch_swizzle.zig").BatchSwizzler;
pub const SwizzleResult = @import("batch_swizzle.zig").SwizzleResult;
pub const BatchStats = @import("batch_swizzle.zig").BatchStats;
