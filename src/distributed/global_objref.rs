// GlobalObjRef: 128-bit Distributed Pointers
//
// This module implements GlobalObjRef, extending the current 64-bit ObjRef
// to support distributed object references across shards.

use crate::distributed::consistent_hash::ShardId;
use crate::document::ObjRef;
use serde::{Deserialize, Serialize};
use std::fmt;

/// 128-bit global object reference for distributed navigation
///
/// Memory Layout: [Shard ID: 32][Local Offset: 64][Flags: 32]
/// - shard_id: Which shard owns this object (0-4B shards supported)
/// - local_offset: Offset within shard's arena (same as ObjRef.offset)
/// - flags: Compression, type hints, version info, etc.
///
/// Design Goals:
/// - Backward compatible with existing ObjRef (local_offset field)
/// - O(1) navigation performance for local references (<50ns)
/// - Cross-shard navigation in <1μs per hop
/// - Efficient encoding for network transmission
///
/// # Example
///
/// ```
/// use oql::distributed::GlobalObjRef;
/// use oql::document::ObjRef;
///
/// // Create from local reference
/// let local_ref = ObjRef::new(1024);
/// let global_ref = GlobalObjRef::from_local(local_ref, 0);
///
/// // Check if local to current shard
/// assert!(global_ref.is_local(0));
///
/// // Convert back to local reference
/// let local_again = global_ref.to_local();
/// assert_eq!(local_again.offset(), 1024);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct GlobalObjRef {
    /// Shard identifier (32 bits = 4 billion shards)
    pub shard_id: ShardId,
    /// Local offset within shard's arena (64 bits, same as ObjRef)
    pub local_offset: u64,
    /// Flags for metadata (32 bits)
    pub flags: u32,
}

/// Flags for GlobalObjRef metadata
pub mod flags {
    /// Object is compressed with LZ4
    pub const COMPRESSED_LZ4: u32 = 0x0001;
    /// Object is compressed with ZSTD
    pub const COMPRESSED_ZSTD: u32 = 0x0002;
    /// Object is pinned in memory (hot data)
    pub const PINNED: u32 = 0x0004;
    /// Object has been migrated (stale reference)
    pub const MIGRATED: u32 = 0x0008;
    /// Object is a schema definition
    pub const SCHEMA: u32 = 0x0010;
    /// Object is an index entry
    pub const INDEX: u32 = 0x0020;
    /// Object is part of a transaction
    pub const TRANSACTIONAL: u32 = 0x0040;
    /// Object is replicated across multiple shards
    pub const REPLICATED: u32 = 0x0080;
}

impl GlobalObjRef {
    /// Create a new GlobalObjRef
    ///
    /// # Arguments
    /// * `shard_id` - Shard that owns this object
    /// * `local_offset` - Offset within the shard's arena
    /// * `flags` - Metadata flags
    pub fn new(shard_id: ShardId, local_offset: u64, flags: u32) -> Self {
        Self {
            shard_id,
            local_offset,
            flags,
        }
    }

    /// Create a GlobalObjRef from a local ObjRef
    ///
    /// # Arguments
    /// * `local_ref` - Local object reference
    /// * `shard_id` - Current shard ID
    pub fn from_local(local_ref: ObjRef, shard_id: ShardId) -> Self {
        Self {
            shard_id,
            local_offset: local_ref.offset(),
            flags: 0,
        }
    }

    /// Create a null GlobalObjRef
    pub fn null() -> Self {
        Self {
            shard_id: 0,
            local_offset: 0,
            flags: 0,
        }
    }

    /// Check if this is a null reference
    pub fn is_null(&self) -> bool {
        self.local_offset == 0
    }

    /// Check if reference is local to the current shard
    ///
    /// # Arguments
    /// * `current_shard` - ID of the current shard
    ///
    /// # Returns
    /// * `true` if object is on the current shard (O(1) navigation)
    /// * `false` if object is on a remote shard (network call required)
    pub fn is_local(&self, current_shard: ShardId) -> bool {
        self.shard_id == current_shard
    }

    /// Convert to local ObjRef for same-shard navigation
    ///
    /// This enables O(1) navigation when the object is on the current shard.
    /// The returned ObjRef can be used with existing arena operations.
    pub fn to_local(&self) -> ObjRef {
        ObjRef::new(self.local_offset)
    }

    /// Get the shard ID
    pub fn shard_id(&self) -> ShardId {
        self.shard_id
    }

    /// Get the local offset
    pub fn local_offset(&self) -> u64 {
        self.local_offset
    }

    /// Get the flags
    pub fn flags(&self) -> u32 {
        self.flags
    }

    /// Set a flag
    pub fn set_flag(&mut self, flag: u32) {
        self.flags |= flag;
    }

    /// Clear a flag
    pub fn clear_flag(&mut self, flag: u32) {
        self.flags &= !flag;
    }

    /// Check if a flag is set
    pub fn has_flag(&self, flag: u32) -> bool {
        (self.flags & flag) != 0
    }

    /// Check if object is compressed
    pub fn is_compressed(&self) -> bool {
        self.has_flag(flags::COMPRESSED_LZ4) || self.has_flag(flags::COMPRESSED_ZSTD)
    }

    /// Check if object is pinned in memory
    pub fn is_pinned(&self) -> bool {
        self.has_flag(flags::PINNED)
    }

    /// Check if object has been migrated (stale reference)
    pub fn is_migrated(&self) -> bool {
        self.has_flag(flags::MIGRATED)
    }

    /// Encode as 128-bit value for network transmission
    ///
    /// Layout: [shard_id: 32][local_offset: 64][flags: 32]
    /// Returns (high_64_bits, low_64_bits)
    pub fn encode(&self) -> (u64, u64) {
        let high = ((self.shard_id as u64) << 32) | ((self.local_offset >> 32) & 0xFFFFFFFF);
        let low = ((self.local_offset & 0xFFFFFFFF) << 32) | (self.flags as u64);
        (high, low)
    }

    /// Decode from 128-bit value
    ///
    /// # Arguments
    /// * `high` - High 64 bits
    /// * `low` - Low 64 bits
    pub fn decode(high: u64, low: u64) -> Self {
        let shard_id = (high >> 32) as u32;
        let local_offset = ((high & 0xFFFFFFFF) << 32) | (low >> 32);
        let flags = (low & 0xFFFFFFFF) as u32;

        Self {
            shard_id,
            local_offset,
            flags,
        }
    }

    /// Encode as bytes for storage
    pub fn to_bytes(&self) -> [u8; 16] {
        let (high, low) = self.encode();
        let mut bytes = [0u8; 16];
        bytes[0..8].copy_from_slice(&high.to_le_bytes());
        bytes[8..16].copy_from_slice(&low.to_le_bytes());
        bytes
    }

    /// Decode from bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() != 16 {
            return Err("GlobalObjRef requires exactly 16 bytes");
        }

        let high = u64::from_le_bytes([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        ]);
        let low = u64::from_le_bytes([
            bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
        ]);

        Ok(Self::decode(high, low))
    }

    /// Get page ID for co-location (same as ObjRef)
    pub fn page_id(&self) -> u32 {
        (self.local_offset / 4096) as u32
    }

    /// Get slot ID within page (same as ObjRef)
    pub fn slot_id(&self) -> u16 {
        ((self.local_offset % 4096) / 64) as u16
    }

    /// Create a GlobalObjRef with updated shard (for migration)
    pub fn with_shard(&self, new_shard_id: ShardId) -> Self {
        Self {
            shard_id: new_shard_id,
            local_offset: self.local_offset,
            flags: self.flags,
        }
    }

    /// Create a GlobalObjRef with updated flags
    pub fn with_flags(&self, new_flags: u32) -> Self {
        Self {
            shard_id: self.shard_id,
            local_offset: self.local_offset,
            flags: new_flags,
        }
    }

    /// Stable string form for APIs and persistence edges.
    pub fn to_string_id(&self) -> String {
        if self.is_null() {
            String::new()
        } else {
            format!(
                "gref_{:08x}_{:016x}_{:08x}",
                self.shard_id, self.local_offset, self.flags
            )
        }
    }

    /// Parse the stable string form produced by `to_string_id`.
    pub fn from_string(s: &str) -> Option<Self> {
        if s.is_empty() {
            return Some(Self::null());
        }
        let rest = s.strip_prefix("gref_")?;
        let mut parts = rest.split('_');
        let shard = u32::from_str_radix(parts.next()?, 16).ok()?;
        let offset = u64::from_str_radix(parts.next()?, 16).ok()?;
        let flags = u32::from_str_radix(parts.next()?, 16).ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self::new(shard, offset, flags))
    }
}

impl fmt::Display for GlobalObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            write!(f, "GlobalObjRef(null)")
        } else {
            write!(
                f,
                "GlobalObjRef(shard={}, offset={}, flags=0x{:08x})",
                self.shard_id, self.local_offset, self.flags
            )
        }
    }
}

impl From<ObjRef> for GlobalObjRef {
    /// Convert local ObjRef to GlobalObjRef (assumes shard 0)
    fn from(local_ref: ObjRef) -> Self {
        Self::from_local(local_ref, 0)
    }
}

impl From<GlobalObjRef> for ObjRef {
    /// Convert GlobalObjRef to local ObjRef (loses shard information)
    fn from(global_ref: GlobalObjRef) -> Self {
        global_ref.to_local()
    }
}

/// Navigation result for cross-shard operations
#[derive(Debug, Clone)]
pub enum NavigationResult<T> {
    /// Object found locally (fast path)
    Local(T),
    /// Object is on remote shard (requires network call)
    Remote {
        shard_id: ShardId,
        objref: GlobalObjRef,
    },
    /// Reference is null or invalid
    NotFound,
}

impl<T> NavigationResult<T> {
    /// Check if result is local
    pub fn is_local(&self) -> bool {
        matches!(self, NavigationResult::Local(_))
    }

    /// Check if result requires remote access
    pub fn is_remote(&self) -> bool {
        matches!(self, NavigationResult::Remote { .. })
    }

    /// Get the local value if available
    pub fn local(self) -> Option<T> {
        match self {
            NavigationResult::Local(value) => Some(value),
            _ => None,
        }
    }

    /// Get remote shard info if applicable
    pub fn remote_info(&self) -> Option<(ShardId, GlobalObjRef)> {
        match self {
            NavigationResult::Remote { shard_id, objref } => Some((*shard_id, *objref)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_global_objref_creation() {
        let global_ref = GlobalObjRef::new(1, 1024, 0);
        assert_eq!(global_ref.shard_id(), 1);
        assert_eq!(global_ref.local_offset(), 1024);
        assert_eq!(global_ref.flags(), 0);
        assert!(!global_ref.is_null());
    }

    #[test]
    fn test_global_objref_string_roundtrip() {
        let original = GlobalObjRef::new(7, 0x1234_5678_9abc_def0, 0x1020_3040);
        let encoded = original.to_string_id();
        let decoded = GlobalObjRef::from_string(&encoded).expect("decode global ref");
        assert_eq!(decoded, original);
    }

    #[test]
    fn test_null_reference() {
        let null_ref = GlobalObjRef::null();
        assert!(null_ref.is_null());
        assert_eq!(null_ref.shard_id(), 0);
        assert_eq!(null_ref.local_offset(), 0);
    }

    #[test]
    fn test_local_conversion() {
        let local_ref = ObjRef::new(2048);
        let global_ref = GlobalObjRef::from_local(local_ref, 5);

        assert_eq!(global_ref.shard_id(), 5);
        assert_eq!(global_ref.local_offset(), 2048);
        assert!(global_ref.is_local(5));
        assert!(!global_ref.is_local(3));

        let local_again = global_ref.to_local();
        assert_eq!(local_again.offset(), 2048);
    }

    #[test]
    fn test_flags() {
        let mut global_ref = GlobalObjRef::new(1, 1024, 0);

        // Set compression flag
        global_ref.set_flag(flags::COMPRESSED_LZ4);
        assert!(global_ref.has_flag(flags::COMPRESSED_LZ4));
        assert!(global_ref.is_compressed());

        // Set pinned flag
        global_ref.set_flag(flags::PINNED);
        assert!(global_ref.has_flag(flags::PINNED));
        assert!(global_ref.is_pinned());

        // Clear compression flag
        global_ref.clear_flag(flags::COMPRESSED_LZ4);
        assert!(!global_ref.has_flag(flags::COMPRESSED_LZ4));
        assert!(!global_ref.is_compressed());
        assert!(global_ref.is_pinned()); // Should still be pinned
    }

    #[test]
    fn test_encoding_decoding() {
        let original = GlobalObjRef::new(0x12345678, 0xABCDEF0123456789, 0x87654321);

        let (high, low) = original.encode();
        let decoded = GlobalObjRef::decode(high, low);

        assert_eq!(original, decoded);
    }

    #[test]
    fn test_bytes_serialization() {
        let original = GlobalObjRef::new(42, 1024 * 1024, flags::COMPRESSED_ZSTD | flags::PINNED);

        let bytes = original.to_bytes();
        assert_eq!(bytes.len(), 16);

        let decoded = GlobalObjRef::from_bytes(&bytes).unwrap();
        assert_eq!(original, decoded);
    }

    #[test]
    fn test_bytes_invalid_length() {
        let short_bytes = [0u8; 8];
        let result = GlobalObjRef::from_bytes(&short_bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_page_and_slot() {
        let global_ref = GlobalObjRef::new(1, 4096 + 128, 0);

        assert_eq!(global_ref.page_id(), 1);
        assert_eq!(global_ref.slot_id(), 2); // 128 / 64 = 2
    }

    #[test]
    fn test_with_methods() {
        let original = GlobalObjRef::new(1, 1024, flags::PINNED);

        let with_new_shard = original.with_shard(5);
        assert_eq!(with_new_shard.shard_id(), 5);
        assert_eq!(with_new_shard.local_offset(), 1024);
        assert_eq!(with_new_shard.flags(), flags::PINNED);

        let with_new_flags = original.with_flags(flags::COMPRESSED_LZ4);
        assert_eq!(with_new_flags.shard_id(), 1);
        assert_eq!(with_new_flags.local_offset(), 1024);
        assert_eq!(with_new_flags.flags(), flags::COMPRESSED_LZ4);
    }

    #[test]
    fn test_display() {
        let null_ref = GlobalObjRef::null();
        assert_eq!(format!("{}", null_ref), "GlobalObjRef(null)");

        let ref_with_flags = GlobalObjRef::new(1, 1024, 0x12345678);
        let display = format!("{}", ref_with_flags);
        assert!(display.contains("shard=1"));
        assert!(display.contains("offset=1024"));
        assert!(display.contains("flags=0x12345678"));
    }

    #[test]
    fn test_from_conversions() {
        let local_ref = ObjRef::new(2048);
        let global_ref: GlobalObjRef = local_ref.into();

        assert_eq!(global_ref.shard_id(), 0); // Default shard
        assert_eq!(global_ref.local_offset(), 2048);

        let local_again: ObjRef = global_ref.into();
        assert_eq!(local_again.offset(), 2048);
    }

    #[test]
    fn test_navigation_result() {
        let local_data = "local_object";
        let local_result = NavigationResult::Local(local_data);

        assert!(local_result.is_local());
        assert!(!local_result.is_remote());
        assert_eq!(local_result.local(), Some("local_object"));

        let global_ref = GlobalObjRef::new(5, 1024, 0);
        let remote_result: NavigationResult<String> = NavigationResult::Remote {
            shard_id: 5,
            objref: global_ref,
        };

        assert!(!remote_result.is_local());
        assert!(remote_result.is_remote());
        assert_eq!(remote_result.remote_info(), Some((5, global_ref)));

        let not_found: NavigationResult<String> = NavigationResult::NotFound;
        assert!(!not_found.is_local());
        assert!(!not_found.is_remote());
    }

    #[test]
    fn test_compression_flags() {
        let mut global_ref = GlobalObjRef::new(1, 1024, 0);

        // No compression initially
        assert!(!global_ref.is_compressed());

        // LZ4 compression
        global_ref.set_flag(flags::COMPRESSED_LZ4);
        assert!(global_ref.is_compressed());

        // Clear LZ4, set ZSTD
        global_ref.clear_flag(flags::COMPRESSED_LZ4);
        global_ref.set_flag(flags::COMPRESSED_ZSTD);
        assert!(global_ref.is_compressed());

        // Clear all compression
        global_ref.clear_flag(flags::COMPRESSED_ZSTD);
        assert!(!global_ref.is_compressed());
    }

    #[test]
    fn test_migration_flag() {
        let mut global_ref = GlobalObjRef::new(1, 1024, 0);

        assert!(!global_ref.is_migrated());

        global_ref.set_flag(flags::MIGRATED);
        assert!(global_ref.is_migrated());
    }

    #[test]
    fn test_large_values() {
        // Test with maximum values
        let max_shard = u32::MAX;
        let max_offset = u64::MAX;
        let max_flags = u32::MAX;

        let global_ref = GlobalObjRef::new(max_shard, max_offset, max_flags);

        let (high, low) = global_ref.encode();
        let decoded = GlobalObjRef::decode(high, low);

        assert_eq!(global_ref, decoded);

        let bytes = global_ref.to_bytes();
        let from_bytes = GlobalObjRef::from_bytes(&bytes).unwrap();
        assert_eq!(global_ref, from_bytes);
    }
}
