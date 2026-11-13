//! Rust-owned storage primitives.
//!
//! This module is the only home for Owl's physical storage implementation.
//! Query and document layers may depend on these stable types, but storage must
//! not depend on an FFI allocator or expose physical file offsets as identity.

mod arena;
mod btree;
mod checkpoint;
mod column_segment;
mod cow;
mod delta;
mod group_commit;
mod index_delta;
mod key;
mod object_ref;
mod record;
mod store;
pub mod vector;

pub(crate) use arena::Arena;
pub use btree::BTreeIndex;
pub use checkpoint::CheckpointInfo;
pub(crate) use column_segment::{
    TypedCell, TypedSegment, TypedSegmentDescriptor, TypedSegmentIdentity,
};
pub use group_commit::GroupCommitter;
pub use key::TypedKey;
pub use object_ref::ObjRef;
pub(crate) use record::{BorrowedRecordValue, CompiledSchema, RecordCodec, RecordView};
pub use store::{
    AutoCheckpointPolicy, IntegrityReport, MaintenanceStatus, ReclamationReport, RustStore,
    StorageLayoutStats, WriteOperation,
};
