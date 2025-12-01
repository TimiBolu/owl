// Physical Execution Module
// Iterator-based execution of physical operators

pub mod operator;
pub mod scan;
pub mod index_scan;
pub mod join;
pub mod aggregate;
pub mod thread_pool;
pub mod parallel_scan;
pub mod parallel_aggregate;

pub use operator::PhysicalOperatorExec;
pub use scan::{SeqScanExec, FilterExec, ProjectExec, SortExec, LimitExec};
pub use index_scan::IndexScanExec;
pub use join::{NestedLoopJoinExec, HashJoinExec};
pub use aggregate::{GroupByExec, HavingExec, AggregateExec};
pub use thread_pool::ThreadPool;
pub use parallel_scan::{ParallelSeqScanExec, ParallelFilterExec};
pub use parallel_aggregate::ParallelAggregateExec;
