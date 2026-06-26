// Core OQL Database System Library
#![allow(clippy::all)]

pub mod change_streams;
pub mod concurrency;
pub mod config;
pub mod distributed;
pub mod document;
pub mod error;
pub mod execution;
pub mod executor;
pub mod functions;
pub mod hot_path_optimizations;
pub mod index_manager;
pub mod lockfree_concurrency;
pub mod monitoring;
pub mod optimizer;
pub mod oql_runner;
pub mod parser;
pub mod planner;
pub mod reference;
pub mod schema;
pub mod simulation;
pub mod storage;
pub mod streaming;
// pub mod transaction; // Deprecated RocksDB-era layer; pending Zig-native redesign.
pub mod performance;
pub mod type_syntax;

// Task 13: Observability and monitoring
pub mod logging;
pub mod observability;
pub mod profiling;

// Task 11: Columnar projections for analytical workloads
pub mod columnar_planner;
pub mod columnar_tracker;

// Task 12: Batch operations for efficient bulk inserts
pub mod batch_operations;

// Task 14: Backup and recovery coordination
pub mod backup_coordinator;

// Task 16: Admin tools for cluster management
pub mod admin;

#[cfg(test)]
mod implicit_null_test;

pub use change_streams::{
    ChangeFilter, ChangeStreamHandle, ChangeStreamManager, ChangeTrackingStore, ChangeType,
    DocumentChange, Trigger, TriggerAction,
};
pub use config::{
    BackupInfo, BackupManager, ConfigFormat, DatabaseConfig, Environment, LoggingConfig,
    MigrationManager, MigrationResult, NetworkConfig, PerformanceConfig, SecurityConfig,
    StorageConfig,
};
pub use document::{Document, DocumentId, Value};
pub use error::{OQLError, OQLResult};
pub use executor::QueryExecutor;
pub use functions::{BuiltinFunction, FunctionRegistry};
pub use monitoring::{
    Alert, AlertSeverity, DashboardData, MonitoringConfig, MonitoringSystem, QueryMetrics,
    QueryStats as MonitoringQueryStats, SystemMetrics, TraceSpan,
};
pub use parser::{
    execute_oql_file, parse_oql, parse_oql_file, parse_oql_statement,
    parse_oql_statement_with_schema, parse_oql_with_schema, ComparisonOperator, Condition,
    Expression, FieldAccess, Literal, OQLFile, OQLFileExecutionResult, OQLFileStatement, OQLQuery,
    Operation, ParseError, Parser, SchemaDefinition, SchemaField, SelectBlock, SelectField,
    StatementResult, Token, TokenizeError, Tokenizer, WhereBlock,
};
pub use reference::{create_reference_id, parse_field_path, ReferenceResolver};
pub use schema::{
    evolution::{
        CompatibilityCheck, CompatibilityLevel, EvolutionError, FieldChange, SchemaEvolution,
        SchemaVersion,
    },
    migration::{
        FieldTransform, Migration, MigrationExecutor, MigrationPlan, MigrationScript,
        MigrationStatus, MigrationStep,
    },
    CascadeBehavior, FieldType, ReferenceType, Schema, SchemaRegistry,
};
pub use storage::DocumentStore;
// pub use transaction::{TransactionManager, TransactionalDocumentStore, IsolationLevel, TransactionContext, TransactionOperation}; // TODO
// pub use index::{IndexManager, IndexDefinition, IndexType, IndexRangeType, IndexUsageStats}; // TODO
pub use optimizer::{
    compare_values, estimate_range_selectivity, estimate_selectivity_from_histogram,
    interpolate_value, FieldStatistics, Histogram, HistogramBucket, HistogramBuilder,
    HistogramType, IndexStatistics, StatisticsConfig, StatisticsManager, TableStatistics,
};
pub use oql_runner::{create_oql_runner, execute_oql_file_simple, OQLRunner};
pub use simulation::controller::{
    create_production_environment, create_simulation, ProductionEnvironment,
};
pub use simulation::{
    Clock, Duration, FileSystem, Instant, Network, RealClock, RealFileSystem, RealNetwork,
    RealScheduler, Scheduler, SimulatedClock, SimulatedFileSystem, SimulatedNetwork,
    SimulatedScheduler, Simulation,
};
pub use streaming::{
    BatchProcessor, PageInfo, PaginatedResult, QueryStream, StreamConfig, StreamCursor,
    StreamingExecutor,
};
