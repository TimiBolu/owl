// Root module that exports all public APIs

pub const ObjRef = @import("objref.zig").ObjRef;
pub const GlobalObjRef = @import("global_objref.zig").GlobalObjRef;
pub const ShardId = @import("global_objref.zig").ShardId;
pub const NavigationResult = @import("global_objref.zig").NavigationResult;
pub const ObjectHeader = @import("object_header.zig").ObjectHeader;
pub const Object = @import("object.zig").Object;
pub const Arena = @import("arena.zig").Arena;
pub const ArenaStats = @import("arena.zig").ArenaStats;
pub const CoLocationMetrics = @import("arena.zig").CoLocationMetrics;
pub const PAGE_SIZE = @import("arena.zig").PAGE_SIZE;
pub const SLOT_SIZE = @import("arena.zig").SLOT_SIZE;

// Deterministic Arena exports (Task 2.2.1)
pub const DeterministicArena = @import("deterministic_arena.zig").DeterministicArena;
pub const DeterministicRNG = @import("deterministic_arena.zig").DeterministicRNG;
pub const AllocationStrategy = @import("deterministic_arena.zig").AllocationStrategy;
pub const GCConfig = @import("deterministic_arena.zig").GCConfig;
pub const AllocationEvent = @import("deterministic_arena.zig").AllocationEvent;

// WAL exports
pub const WAL = @import("wal.zig").WAL;
pub const WALEntry = @import("wal.zig").WALEntry;
pub const LSN = @import("wal.zig").LSN;
pub const EntryType = @import("wal.zig").EntryType;
pub const WALStats = @import("wal.zig").WALStats;

// Lock-Free WAL exports (Task 9.4)
pub const LockFreeWAL = @import("lockfree_wal.zig").LockFreeWAL;

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

// B-tree exports
pub const BTree = @import("btree.zig").BTree;
pub const BTreeConfig = @import("btree.zig").BTreeConfig;
pub const BTreeNode = @import("btree.zig").BTreeNode;
pub const Key = @import("btree.zig").Key;

// Garbage Collection exports
pub const GarbageCollector = @import("gc.zig").GarbageCollector;
pub const BackgroundGC = @import("gc.zig").BackgroundGC;
pub const GCStats = @import("gc.zig").GCStats;

// Crash Simulation exports (Task 2.2.2)
pub const CrashSimulator = @import("crash_simulation.zig").CrashSimulator;
pub const CrashAwareArena = @import("crash_simulation.zig").CrashAwareArena;
pub const CrashAwareWAL = @import("crash_simulation.zig").CrashAwareWAL;
pub const FailureConfig = @import("crash_simulation.zig").FailureConfig;
pub const CrashMode = @import("crash_simulation.zig").CrashMode;
pub const CorruptionType = @import("crash_simulation.zig").CorruptionType;
pub const CrashStats = @import("crash_simulation.zig").CrashStats;
pub const CrashRNG = @import("crash_simulation.zig").CrashRNG;

// Distributed Arena exports (Task 8.8)
pub const DistributedArenaManager = @import("distributed_arena.zig").DistributedArenaManager;
pub const ShardStatus = @import("distributed_arena.zig").ShardStatus;
pub const MigrationState = @import("distributed_arena.zig").MigrationState;
pub const ShardStats = @import("distributed_arena.zig").ShardStats;

// Distributed WAL exports (Task 8.10)
pub const DistributedWALEntry = @import("distributed_wal.zig").DistributedWALEntry;
pub const DistributedWALEntryHeader = @import("distributed_wal.zig").DistributedWALEntryHeader;
pub const DistributedEntryType = @import("distributed_wal.zig").DistributedEntryType;
pub const DistributedTxnId = @import("distributed_wal.zig").DistributedTxnId;
pub const ShardWALManager = @import("distributed_wal.zig").ShardWALManager;
pub const DistributedTransaction = @import("distributed_wal.zig").DistributedTransaction;
pub const DistributedTransactionState = @import("distributed_wal.zig").TransactionState;
pub const WALReplicationManager = @import("distributed_wal.zig").WALReplicationManager;
pub const ShardSynchronizationManager = @import("distributed_wal.zig").ShardSynchronizationManager;
pub const SyncState = @import("distributed_wal.zig").SyncState;

// Simulation Integration exports (Task 8.11)
pub const SimulatedStorageEngine = @import("simulation_integration_simple.zig").SimulatedStorageEngine;
pub const SimulationMode = @import("simulation_integration_simple.zig").SimulationMode;
pub const SimulationConfig = @import("simulation_integration_simple.zig").SimulationConfig;
pub const SimulationStats = @import("simulation_integration_simple.zig").SimulationStats;
pub const SimulationEngineStats = @import("simulation_integration_simple.zig").SimulationEngineStats;

// Concurrency Primitives exports (Task 8.12)
pub const AtomicRefCount = @import("concurrency_primitives.zig").AtomicRefCount;
pub const SharedObject = @import("concurrency_primitives.zig").SharedObject;
pub const HazardPointerRecord = @import("concurrency_primitives.zig").HazardPointerRecord;
pub const HazardPointerManager = @import("concurrency_primitives.zig").HazardPointerManager;
pub const LockFreeStack = @import("concurrency_primitives.zig").LockFreeStack;
pub const LockFreeQueue = @import("concurrency_primitives.zig").LockFreeQueue;
pub const AtomicObjectHeader = @import("concurrency_primitives.zig").AtomicObjectHeader;
pub const MAX_HAZARD_POINTERS = @import("concurrency_primitives.zig").MAX_HAZARD_POINTERS;

// Transaction FFI exports (Task 8.13)
pub const zig_transaction_ffi_init = @import("transaction_ffi.zig").zig_transaction_ffi_init;
pub const zig_transaction_ffi_cleanup = @import("transaction_ffi.zig").zig_transaction_ffi_cleanup;
pub const zig_transaction_begin = @import("transaction_ffi.zig").zig_transaction_begin;
pub const zig_transaction_prepare = @import("transaction_ffi.zig").zig_transaction_prepare;
pub const zig_transaction_commit = @import("transaction_ffi.zig").zig_transaction_commit;
pub const zig_transaction_abort = @import("transaction_ffi.zig").zig_transaction_abort;
pub const zig_transaction_get_state = @import("transaction_ffi.zig").zig_transaction_get_state;
pub const zig_transaction_write_operation = @import("transaction_ffi.zig").zig_transaction_write_operation;

// Backup and Recovery exports (Task 8.16)
pub const BackupRecovery = @import("backup_recovery.zig").BackupRecovery;
pub const BackupMetadata = @import("backup_recovery.zig").BackupMetadata;
pub const BackupType = @import("backup_recovery.zig").BackupType;
pub const CompressionType = @import("backup_recovery.zig").CompressionType;
pub const StorageBackend = @import("backup_recovery.zig").StorageBackend;
pub const BackupEntry = @import("backup_recovery.zig").BackupEntry;
pub const BackupStats = @import("backup_recovery.zig").BackupStats;
pub const RecoveryOptions = @import("backup_recovery.zig").RecoveryOptions;
pub const BackupConfig = @import("backup_recovery.zig").BackupConfig;

// SIMD Operations exports (Task 9.8)
pub const simd_ops = @import("simd_ops.zig");
pub const simdMemcpy = simd_ops.simdMemcpy;
pub const simdMemcmp = simd_ops.simdMemcmp;
pub const simdCRC32 = simd_ops.simdCRC32;
pub const simdStrlen = simd_ops.simdStrlen;
pub const simdStrcmp = simd_ops.simdStrcmp;
pub const simdStrstr = simd_ops.simdStrstr;
pub const simdMemset = simd_ops.simdMemset;
pub const simdCountByte = simd_ops.simdCountByte;

// Memory Manager exports (Task 9.10)
pub const MemoryManager = @import("memory_manager.zig").MemoryManager;
pub const MemoryManagerConfig = @import("memory_manager.zig").MemoryManagerConfig;
pub const MemoryStats = @import("memory_manager.zig").MemoryStats;
pub const MemoryAllocationStrategy = @import("memory_manager.zig").AllocationStrategy;
pub const CompactionPolicy = @import("memory_manager.zig").CompactionPolicy;
pub const NumaPolicy = @import("memory_manager.zig").NumaPolicy;
pub const NumaNode = @import("memory_manager.zig").NumaNode;

// Compression exports (Task 10)
pub const compression = @import("compression.zig");
pub const CompressionEngine = compression.CompressionEngine;
pub const CompressionStats = compression.CompressionStats;
pub const CompressedHeader = compression.CompressedHeader;
pub const AdaptiveCompressionType = compression.CompressionType;

// Decompression Cache exports (Task 10.1)
pub const DecompressionCache = @import("decompression_cache.zig").DecompressionCache;
pub const DecompressionCacheStats = @import("decompression_cache.zig").CacheStats;

// Columnar Projections exports (Task 11)
pub const columnar = @import("columnar.zig");
pub const ColumnType = columnar.ColumnType;
pub const ColumnCompressionType = columnar.ColumnCompressionType;
pub const ColumnMetadata = columnar.ColumnMetadata;
pub const ColumnarProjection = columnar.ColumnarProjection;
pub const ColumnarProjectionBuilder = columnar.ColumnarProjectionBuilder;
pub const ColumnarProjectionManager = columnar.ColumnarProjectionManager;
pub const VectorizedScanner = columnar.VectorizedScanner;

// Columnar Compression exports (Task 11.2)
pub const columnar_compression = @import("columnar_compression.zig");
pub const RLECompressor = columnar_compression.RLECompressor;
pub const DeltaCompressor = columnar_compression.DeltaCompressor;
pub const DictionaryCompressor = columnar_compression.DictionaryCompressor;
pub const BitPackCompressor = columnar_compression.BitPackCompressor;
pub const NullBitmapCompressor = columnar_compression.NullBitmapCompressor;
pub const ColumnarCompressionEngine = columnar_compression.ColumnarCompressionEngine;

// Metrics exports (Task 13.1)
pub const metrics = @import("metrics.zig");
pub const StorageMetrics = metrics.StorageMetrics;
pub const MetricsSnapshot = metrics.MetricsSnapshot;
pub const DerivedMetrics = metrics.DerivedMetrics;
pub const getGlobalMetrics = metrics.getGlobalMetrics;
pub const initGlobalMetrics = metrics.initGlobalMetrics;

// Tracing exports (Task 13.2)
pub const tracing = @import("tracing.zig");
pub const TraceContext = tracing.TraceContext;
pub const TraceSpan = tracing.TraceSpan;
pub const setCurrentContext = tracing.setCurrentContext;
pub const getCurrentContext = tracing.getCurrentContext;
pub const clearCurrentContext = tracing.clearCurrentContext;
pub const startSpan = tracing.startSpan;

// Logging exports (Task 13.3)
pub const logging = @import("logging.zig");
pub const LogLevel = logging.LogLevel;
pub const CorrelationId = logging.CorrelationId;
pub const LogEntry = logging.LogEntry;
pub const Logger = logging.Logger;
pub const TimedOperation = logging.TimedOperation;
pub const setCurrentLogger = logging.setCurrentLogger;
pub const getCurrentLogger = logging.getCurrentLogger;
pub const clearCurrentLogger = logging.clearCurrentLogger;

// Profiling exports (Task 13.4)
pub const profiling = @import("profiling.zig");
pub const ProfileSample = profiling.ProfileSample;
pub const ProfiledFunction = profiling.ProfiledFunction;
pub const recordAllocation = profiling.recordAllocation;
