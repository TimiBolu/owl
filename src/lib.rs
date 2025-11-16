// Core OQL Database System Library

pub mod change_streams;
pub mod concurrency;
pub mod config;
pub mod document;
pub mod error;
pub mod executor;
pub mod functions;
// pub mod index; // TODO: Reimplement with Zig storage
pub mod monitoring;
// pub mod optimizer; // TODO: Reimplement after index module
pub mod oql_runner;
pub mod parser;
pub mod reference;
pub mod schema;
pub mod storage;
pub mod storage_engine;
pub mod streaming;
// pub mod transaction; // TODO: Reimplement with Zig transaction layer
pub mod performance;



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
pub use schema::{FieldType, Schema, ReferenceType, CascadeBehavior, SchemaRegistry};
pub use storage::DocumentStore;
// pub use transaction::{TransactionManager, TransactionalDocumentStore, IsolationLevel, TransactionContext, TransactionOperation}; // TODO
// pub use index::{IndexManager, IndexDefinition, IndexType, IndexRangeType, IndexUsageStats}; // TODO
// pub use optimizer::{QueryOptimizer, QueryPlan, IndexUsage, IndexRecommendation, QueryStats, CacheStats}; // TODO
pub use streaming::{QueryStream, StreamingExecutor, StreamCursor, PaginatedResult, StreamConfig, BatchProcessor, PageInfo};
pub use oql_runner::{OQLRunner, execute_oql_file_simple};
pub use storage_engine::{
    Object, ObjectHeader, ObjRef, VarData, ObjectStore, StorageStats, 
    StorageError, StorageResult
};