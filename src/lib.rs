// Core OQL Database System Library

pub mod change_streams;
pub mod concurrency;
pub mod lockfree_concurrency;
pub mod config;
pub mod distributed;
pub mod document;
pub mod error;
pub mod execution;
pub mod executor;
pub mod functions;
pub mod hot_path_optimizations;
pub mod index_manager;
pub mod monitoring;
pub mod optimizer;
pub mod oql_runner;
pub mod parser;
pub mod planner;
pub mod reference;
pub mod schema;
pub mod simulation;
pub mod storage;
pub mod storage_engine;
pub mod streaming;
// pub mod transaction; // TODO: Reimplement with Zig transaction layer
pub mod performance;

// Task 8.13: Rust-Zig transaction coordination modules
pub mod distributed_transaction_coordinator;
pub mod cross_layer_error_handling;
pub mod acid_coordinator;

// Task 8.15: Cross-layer observability module
pub mod cross_layer_observability;

// Task 13: Observability and monitoring
pub mod observability;
pub mod logging;
pub mod profiling;

// Task 11: Columnar projections for analytical workloads
pub mod columnar_tracker;
pub mod columnar_planner;

// Task 12: Batch operations for efficient bulk inserts
pub mod batch_operations;

// Task 14: Backup and recovery coordination
pub mod backup_coordinator;

// Task 16: Admin tools for cluster management
pub mod admin;



#[cfg(test)]
mod implicit_null_test;

pub use change_streams::{
    ChangeStreamManager, ChangeTrackingStore, DocumentChange, ChangeType, ChangeFilter, 
    ChangeStreamHandle, Trigger, TriggerAction
};
pub use config::{
    DatabaseConfig, StorageConfig, PerformanceConfig, SecurityConfig, NetworkConfig,
    LoggingConfig, Environment, BackupManager, BackupInfo, MigrationManager, MigrationResult,
    ConfigFormat
};
pub use document::{Document, DocumentId, Value};
pub use error::{OQLError, OQLResult};
pub use executor::QueryExecutor;
pub use functions::{FunctionRegistry, BuiltinFunction};
pub use monitoring::{
    MonitoringSystem, MonitoringConfig, QueryMetrics, SystemMetrics, Alert, AlertSeverity,
    TraceSpan, DashboardData, QueryStats as MonitoringQueryStats
};
pub use parser::{
    parse_oql, parse_oql_with_schema, parse_oql_statement, parse_oql_statement_with_schema,
    parse_oql_file, execute_oql_file, ComparisonOperator, Condition, Expression, 
    FieldAccess, Literal, OQLQuery, Operation, ParseError, Parser, SelectBlock, SelectField, 
    Token, Tokenizer, TokenizeError, WhereBlock, OQLFile, OQLFileStatement, SchemaDefinition, 
    SchemaField, OQLFileExecutionResult, StatementResult,
};
pub use reference::{ReferenceResolver, parse_field_path, create_reference_id};
pub use schema::{
    FieldType, Schema, ReferenceType, CascadeBehavior, SchemaRegistry,
    evolution::{SchemaVersion, SchemaEvolution, FieldChange, CompatibilityCheck, CompatibilityLevel, EvolutionError},
    migration::{Migration, MigrationPlan, MigrationStep, MigrationScript, MigrationExecutor, MigrationStatus, FieldTransform},
};
pub use storage::DocumentStore;
// pub use transaction::{TransactionManager, TransactionalDocumentStore, IsolationLevel, TransactionContext, TransactionOperation}; // TODO
// pub use index::{IndexManager, IndexDefinition, IndexType, IndexRangeType, IndexUsageStats}; // TODO
pub use optimizer::{
    StatisticsManager, TableStatistics, FieldStatistics, IndexStatistics,
    StatisticsConfig, Histogram, HistogramBucket, HistogramType,
    HistogramBuilder, compare_values, interpolate_value,
    estimate_selectivity_from_histogram, estimate_range_selectivity,
};
pub use streaming::{QueryStream, StreamingExecutor, StreamCursor, PaginatedResult, StreamConfig, BatchProcessor, PageInfo};
pub use oql_runner::{OQLRunner, execute_oql_file_simple, create_oql_runner};
pub use simulation::{
    Clock, Scheduler, FileSystem, Network, Simulation,
    SimulatedClock, RealClock, SimulatedScheduler, RealScheduler,
    SimulatedFileSystem, RealFileSystem, SimulatedNetwork, RealNetwork,
    Duration, Instant
};
pub use simulation::controller::{ProductionEnvironment, create_simulation, create_production_environment};
pub use storage_engine::{
    Object, ObjectHeader, ObjRef, VarData, ObjectStore, StorageStats, 
    StorageError, StorageResult
};

// Task 8.13: Rust-Zig transaction coordination exports
pub use distributed_transaction_coordinator::{
    DistributedTransactionCoordinator, DistributedTxnId, DistributedTxnState,
    ParticipantType, TransactionParticipant, DistributedTransaction,
    CoordinatorConfig, CoordinatorStats
};
pub use cross_layer_error_handling::{
    CrossLayerErrorManager, ZigError, ZigErrorCode, ErrorStatistics,
    ErrorRecoveryStrategy, RecoveryAction
};
pub use acid_coordinator::{
    ACIDCoordinator, ACIDValidationResult, ACIDCoordinatorConfig,
    ACIDContext, ACIDOperation, OperationType, ConsistencyConstraint,
    ConstraintType, DurabilityCheckpoint, ACIDStatistics
};

// Task 8.15: Cross-layer observability exports
pub use cross_layer_observability::{
    CrossLayerObservabilityManager, ZigStorageMetrics, ArenaMetrics, BTreeMetrics,
    WALMetrics, TransactionMetrics, MemoryMetrics, IOMetrics, CrossLayerTraceContext,
    CrossLayerSpan, ZigTraceSpan, PerformanceSummary, UnifiedLogEntry, LogLevel,
    ComprehensiveMetrics, ObservabilityDashboard
};