// Physical Execution Module
// Iterator-based execution of physical operators

pub mod aggregate;
pub mod index_scan;
pub mod join;
pub mod operator;
pub mod parallel_aggregate;
pub mod parallel_scan;
pub mod scan;
pub mod thread_pool;

pub use aggregate::{AggregateExec, GroupByExec, HavingExec};
pub use index_scan::IndexScanExec;
pub use join::{HashJoinExec, NestedLoopJoinExec};
pub use operator::PhysicalOperatorExec;
pub use parallel_aggregate::ParallelAggregateExec;
pub use parallel_scan::{ParallelFilterExec, ParallelSeqScanExec};
pub use scan::{FilterExec, LimitExec, ProjectExec, SeqScanExec, SortExec};
pub use thread_pool::ThreadPool;
