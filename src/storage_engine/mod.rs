//! Rust-owned storage primitives.
//!
//! This module is the only home for Owl's physical storage implementation.
//! Query and document layers may depend on these stable types, but storage must
//! not depend on an FFI allocator or expose physical file offsets as identity.

mod arena;
mod backup;
mod btree;
mod checkpoint;
mod column_segment;
mod cow;
mod delta;
mod group_commit;
mod index_delta;
mod key;
mod object_ref;
mod object_store;
mod parts;
mod record;
mod s3;
mod store;
mod vector_index;

pub(crate) use arena::Arena;
pub use btree::BTreeIndex;
pub use checkpoint::CheckpointInfo;
pub(crate) use column_segment::{
    ColumnBuilder, ColumnValues, TypedCell, TypedColumn, TypedSegment, TypedSegmentDescriptor,
    TypedSegmentIdentity,
};
pub use group_commit::GroupCommitter;
pub use key::TypedKey;
pub use object_ref::ObjRef;
pub use object_store::{
    DirectoryObjectStore, MemoryObjectStore, ObjectFileSystem, ObjectStore, PartCache, Published,
    PublishedFile,
};
pub(crate) use record::{BorrowedRecordValue, CompiledSchema, RecordCodec, RecordView};
pub use backup::{BackupFile, BackupInfo};
pub use s3::{Request, Response, S3Config, S3Faults, S3ObjectStore, SimulatedS3, Transport};
#[cfg(feature = "s3")]
pub use s3::HttpTransport;
pub use vector_index::{divisions as vector_index_divisions, Searched as VectorSearched};
pub(crate) use vector_index::VectorIndex;
pub use store::{
    AutoCheckpointPolicy, IntegrityReport, MaintenanceStatus, ReclamationReport, RustStore,
    StorageLayoutStats, TransactionBinding, WriteOperation,
};
