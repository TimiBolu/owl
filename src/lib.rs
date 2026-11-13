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
pub mod storage_engine;
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
pub use execution::OperatorExecutionStats;
pub use executor::{BatchQueryExecution, QueryExecution, QueryExecutor};
pub use functions::{BuiltinFunction, FunctionRegistry};
pub use monitoring::{
    Alert, AlertSeverity, DashboardData, MonitoringConfig, MonitoringSystem, QueryMetrics,
    QueryStats as MonitoringQueryStats, SystemMetrics, TraceSpan,
};
pub use parser::{
    execute_oql_file, parse_oql, parse_oql_file, parse_oql_statement,
    parse_oql_statement_with_schema, parse_oql_with_schema, resolve_query_with_bindings,
    resolve_query_with_value_bindings, resolve_value_with_bindings, ComparisonOperator, Condition,
    Expression, FieldAccess, Literal, OQLFile, OQLFileExecutionResult, OQLFileStatement, OQLQuery,
    Operation, ParseError, Parser, QueryBinding, SchemaDefinition, SchemaField, SelectBlock,
    SelectField, StatementResult, Token, TokenizeError, Tokenizer, UpsertBlock, ValueBinding,
    WhereBlock,
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
    CascadeBehavior, DocumentSystemField, FieldType, ReferenceType, Schema, SchemaRegistry,
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

/// Parse a static OQL query while keeping Owl syntax at the call site.
///
/// The macro returns the same result as [`parse_oql`]. Use the literal form
/// for long queries and the token form for compact, fixed queries. Runtime
/// values should still be bound through a query builder or safely constructed
/// string rather than interpolated into macro tokens.
///
/// ```
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let errors = owl_db::oql!(events.where { .level == "error" }.limit 20)?;
/// let recent = owl_db::oql!(r#"
///     events
///         .sort { .occurred_at desc }
///         .limit 100
/// "#)?;
/// assert_eq!(errors.collection, "events");
/// assert_eq!(recent.collection, "events");
/// # Ok(())
/// # }
/// ```
#[macro_export]
macro_rules! oql {
    ($query:literal) => {
        $crate::parse_oql($query)
    };
    ($($query:tt)+) => {
        $crate::parse_oql(::core::stringify!($($query)+))
    };
}

#[cfg(test)]
mod oql_macro_tests {
    #[test]
    fn token_form_keeps_oql_at_the_call_site() {
        let query = crate::oql!(events.where { .level == "error" }.limit 20).unwrap();
        assert_eq!(query.collection, "events");
        assert_eq!(query.operations.len(), 2);
    }

    #[test]
    fn literal_form_supports_large_multiline_queries() {
        let query = crate::oql!(
            r#"
            events
                .sort { .occurred_at desc }
                .limit 100
        "#
        )
        .unwrap();
        assert_eq!(query.collection, "events");
        assert_eq!(query.operations.len(), 2);
    }
}
