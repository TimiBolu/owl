// OQL Storage Engine - Core Object Model and Physical References
//
// This module implements the foundational components for the OQL storage engine:
// - Object and ObjectHeader structs with proper memory layout
// - ObjRef (physical pointer) with swizzling methods
// - VarData encoding using CBOR
// - ObjectStore trait for storage operations

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Result type for storage engine operations
pub type StorageResult<T> = Result<T, StorageError>;

/// Storage engine error types
#[derive(Debug, Clone, PartialEq)]
pub enum StorageError {
    /// I/O error
    IOError(String),
    /// Invalid object reference
    InvalidObjRef(ObjRef),
    /// Object not found
    ObjectNotFound(ObjRef),
    /// Allocation failed
    AllocationFailed,
    /// Out of space
    OutOfSpace,
    /// Serialization error
    SerializationError(String),
    /// Checksum mismatch
    ChecksumMismatch { expected: u32, actual: u32 },
}

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StorageError::IOError(msg) => write!(f, "I/O error: {}", msg),
            StorageError::InvalidObjRef(objref) => write!(f, "Invalid object reference: {:?}", objref),
            StorageError::ObjectNotFound(objref) => write!(f, "Object not found: {:?}", objref),
            StorageError::AllocationFailed => write!(f, "Allocation failed"),
            StorageError::OutOfSpace => write!(f, "Out of space"),
            StorageError::SerializationError(msg) => write!(f, "Serialization error: {}", msg),
            StorageError::ChecksumMismatch { expected, actual } => {
                write!(f, "Checksum mismatch: expected {}, got {}", expected, actual)
            }
        }
    }
}

impl std::error::Error for StorageError {}

/// Object header with proper memory layout for cache efficiency
/// Uses #[repr(C)] to ensure predictable memory layout
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObjectHeader {
    /// Schema type ID (0=User, 1=Order, etc.)
    pub type_tag: u16,
    /// Logical identifier (16 bytes)
    pub id: Uuid,
    /// MVCC version number
    pub version: u32,
    /// Total object size in bytes
    pub size: u32,
    /// Flags: Deleted, Overflow, Pinned, etc.
    pub flags: u8,
    /// CRC32 checksum for integrity
    pub checksum: u32,
    /// Log Sequence Number for MVCC
    pub lsn: u64,
}

impl ObjectHeader {
    /// Create a new object header
    pub fn new(type_tag: u16, id: Uuid, size: u32) -> Self {
        Self {
            type_tag,
            id,
            version: 0,
            size,
            flags: 0,
            checksum: 0,
            lsn: 0,
        }
    }

    /// Check if object is marked as deleted
    pub fn is_deleted(&self) -> bool {
        self.flags & 0x01 != 0
    }

    /// Mark object as deleted
    pub fn set_deleted(&mut self, deleted: bool) {
        if deleted {
            self.flags |= 0x01;
        } else {
            self.flags &= !0x01;
        }
    }

    /// Check if object has overflow data
    pub fn has_overflow(&self) -> bool {
        self.flags & 0x02 != 0
    }

    /// Mark object as having overflow data
    pub fn set_overflow(&mut self, overflow: bool) {
        if overflow {
            self.flags |= 0x02;
        } else {
            self.flags &= !0x02;
        }
    }

    /// Check if object is pinned in memory
    pub fn is_pinned(&self) -> bool {
        self.flags & 0x04 != 0
    }

    /// Pin/unpin object in memory
    pub fn set_pinned(&mut self, pinned: bool) {
        if pinned {
            self.flags |= 0x04;
        } else {
            self.flags &= !0x04;
        }
    }

    /// Calculate CRC32 checksum for the header
    pub fn calculate_checksum(&self, data: &[u8]) -> u32 {
        crc32fast::hash(data)
    }

    /// Verify checksum
    pub fn verify_checksum(&self, data: &[u8]) -> bool {
        self.checksum == self.calculate_checksum(data)
    }
}

/// Physical pointer represented as a 64-bit offset into the storage file
/// This enables O(1) navigation through object references
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ObjRef {
    /// File-global offset (8 bytes)
    offset: u64,
}

impl ObjRef {
    /// Create a new object reference from an offset
    pub fn new(offset: u64) -> Self {
        Self { offset }
    }

    /// Create a null reference
    pub fn null() -> Self {
        Self { offset: 0 }
    }

    /// Check if this is a null reference
    pub fn is_null(&self) -> bool {
        self.offset == 0
    }

    /// Get the raw offset value
    pub fn offset(&self) -> u64 {
        self.offset
    }

    /// Swizzle: Convert disk offset to memory pointer
    /// 
    /// # Safety
    /// This is unsafe because it performs pointer arithmetic.
    /// The caller must ensure:
    /// - base pointer is valid
    /// - offset is within bounds
    /// - resulting pointer points to valid Object data
    pub unsafe fn swizzle(&self, base: *mut u8) -> *mut Object {
        base.add(self.offset as usize) as *mut Object
    }

    /// Get the page ID (4KB page number) for this reference
    /// Used for co-location and cache optimization
    pub fn page_id(&self) -> u32 {
        (self.offset / 4096) as u32
    }

    /// Get the slot ID within the page
    /// Assumes 64-byte slots within each 4KB page
    pub fn slot_id(&self) -> u16 {
        ((self.offset % 4096) / 64) as u16
    }

    /// Create an ObjRef from page_id and slot_id
    pub fn from_page_slot(page_id: u32, slot_id: u16) -> Self {
        let offset = (page_id as u64 * 4096) + (slot_id as u64 * 64);
        Self { offset }
    }
}

/// Variable-length data encoded using CBOR for nested fields
/// This allows efficient storage of complex nested structures
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VarData {
    /// CBOR-encoded data
    data: Vec<u8>,
}

impl VarData {
    /// Create empty VarData
    pub fn empty() -> Self {
        Self { data: Vec::new() }
    }

    /// Encode a value into CBOR format
    pub fn encode<T: Serialize>(value: &T) -> StorageResult<Self> {
        let data = serde_cbor::to_vec(value)
            .map_err(|e| StorageError::SerializationError(e.to_string()))?;
        Ok(Self { data })
    }

    /// Decode CBOR data into a value
    pub fn decode<T: for<'de> Deserialize<'de>>(&self) -> StorageResult<T> {
        serde_cbor::from_slice(&self.data)
            .map_err(|e| StorageError::SerializationError(e.to_string()))
    }

    /// Get the raw CBOR data
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Get the size of the encoded data
    pub fn size(&self) -> usize {
        self.data.len()
    }
}

/// Complete object structure with header, fixed fields, variable data, and references
#[derive(Debug, Clone)]
pub struct Object {
    /// Object header with metadata
    pub header: ObjectHeader,
    /// Fixed-size fields (aligned for cache efficiency)
    pub fixed: Vec<u8>,
    /// CBOR-encoded nested/variable data
    pub var: VarData,
    /// Array of references to other objects
    pub refs: Vec<ObjRef>,
}

impl Object {
    /// Create a new object
    pub fn new(type_tag: u16, id: Uuid, fixed: Vec<u8>, var: VarData, refs: Vec<ObjRef>) -> Self {
        let size = std::mem::size_of::<ObjectHeader>() as u32
            + fixed.len() as u32
            + var.size() as u32
            + (refs.len() * std::mem::size_of::<ObjRef>()) as u32;

        let header = ObjectHeader::new(type_tag, id, size);

        Self {
            header,
            fixed,
            var,
            refs,
        }
    }

    /// Get total size of the object in bytes
    pub fn total_size(&self) -> usize {
        self.header.size as usize
    }

    /// Serialize object to bytes for storage
    pub fn to_bytes(&self) -> StorageResult<Vec<u8>> {
        let mut bytes = Vec::with_capacity(self.total_size());

        // Serialize header
        let header_bytes = unsafe {
            std::slice::from_raw_parts(
                &self.header as *const ObjectHeader as *const u8,
                std::mem::size_of::<ObjectHeader>(),
            )
        };
        bytes.extend_from_slice(header_bytes);

        // Serialize fixed fields
        bytes.extend_from_slice(&self.fixed);

        // Serialize variable data
        bytes.extend_from_slice(self.var.as_bytes());

        // Serialize references
        for objref in &self.refs {
            bytes.extend_from_slice(&objref.offset.to_le_bytes());
        }

        Ok(bytes)
    }

    /// Deserialize object from bytes
    pub fn from_bytes(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.len() < std::mem::size_of::<ObjectHeader>() {
            return Err(StorageError::SerializationError(
                "Insufficient data for object header".to_string(),
            ));
        }

        // Deserialize header
        let header = unsafe {
            std::ptr::read(bytes.as_ptr() as *const ObjectHeader)
        };

        let mut offset = std::mem::size_of::<ObjectHeader>();

        // Calculate sizes
        let total_size = header.size as usize;
        if bytes.len() < total_size {
            return Err(StorageError::SerializationError(
                "Insufficient data for complete object".to_string(),
            ));
        }

        // For now, we'll use a simple heuristic to split the remaining data
        // In a real implementation, we'd store size metadata
        let remaining = total_size - offset;
        
        // Assume refs are at the end (8 bytes each)
        let mut refs_count = 0;
        let mut temp_offset = offset;
        while temp_offset + 8 <= total_size {
            // Try to read as potential reference
            temp_offset += 8;
            refs_count += 1;
        }

        // Adjust: assume last N*8 bytes are refs, rest is fixed + var
        let refs_size = refs_count * 8;
        let data_size = remaining - refs_size;

        // For simplicity, assume first half is fixed, second half is var
        let fixed_size = data_size / 2;
        let var_size = data_size - fixed_size;

        // Deserialize fixed fields
        let fixed = bytes[offset..offset + fixed_size].to_vec();
        offset += fixed_size;

        // Deserialize variable data
        let var_bytes = bytes[offset..offset + var_size].to_vec();
        let var = VarData { data: var_bytes };
        offset += var_size;

        // Deserialize references
        let mut refs = Vec::new();
        while offset + 8 <= total_size {
            let ref_offset = u64::from_le_bytes([
                bytes[offset],
                bytes[offset + 1],
                bytes[offset + 2],
                bytes[offset + 3],
                bytes[offset + 4],
                bytes[offset + 5],
                bytes[offset + 6],
                bytes[offset + 7],
            ]);
            refs.push(ObjRef::new(ref_offset));
            offset += 8;
        }

        Ok(Self {
            header,
            fixed,
            var,
            refs,
        })
    }

    /// Update checksum based on current data
    pub fn update_checksum(&mut self) {
        if let Ok(bytes) = self.to_bytes() {
            self.header.checksum = self.header.calculate_checksum(&bytes);
        }
    }

    /// Verify object integrity
    pub fn verify(&self) -> StorageResult<()> {
        if let Ok(bytes) = self.to_bytes() {
            if !self.header.verify_checksum(&bytes) {
                return Err(StorageError::ChecksumMismatch {
                    expected: self.header.checksum,
                    actual: self.header.calculate_checksum(&bytes),
                });
            }
        }
        Ok(())
    }
}

/// ObjectStore trait defining core storage operations
/// This provides the interface for different storage backends (Arena, LSM, etc.)
pub trait ObjectStore {
    /// Allocate space for a new object
    /// 
    /// # Arguments
    /// * `size` - Size of the object in bytes
    /// * `near` - Optional reference to co-locate near (for cache efficiency)
    /// 
    /// # Returns
    /// ObjRef pointing to the allocated space
    fn alloc(&mut self, size: u32, near: Option<ObjRef>) -> StorageResult<ObjRef>;

    /// Write an object to storage
    /// 
    /// # Arguments
    /// * `objref` - Reference to write location
    /// * `data` - Serialized object data
    fn write(&mut self, objref: ObjRef, data: &[u8]) -> StorageResult<()>;

    /// Read an object from storage
    /// 
    /// # Arguments
    /// * `objref` - Reference to read location
    /// 
    /// # Returns
    /// Reference to the object (may be cached)
    fn read(&self, objref: ObjRef) -> StorageResult<&Object>;

    /// Delete an object from storage
    /// 
    /// # Arguments
    /// * `objref` - Reference to delete
    fn delete(&mut self, objref: ObjRef) -> StorageResult<()>;

    /// Get statistics about storage usage
    fn stats(&self) -> StorageStats;
}

/// Storage statistics
#[derive(Debug, Clone, Default)]
pub struct StorageStats {
    /// Total allocated bytes
    pub allocated_bytes: u64,
    /// Used bytes
    pub used_bytes: u64,
    /// Number of objects
    pub object_count: u64,
    /// Number of free slots
    pub free_slots: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_objref_null() {
        let null_ref = ObjRef::null();
        assert!(null_ref.is_null());
        assert_eq!(null_ref.offset(), 0);
    }

    #[test]
    fn test_objref_page_slot() {
        let objref = ObjRef::new(4096 + 128); // Page 1, slot 2
        assert_eq!(objref.page_id(), 1);
        assert_eq!(objref.slot_id(), 2);

        let reconstructed = ObjRef::from_page_slot(1, 2);
        assert_eq!(reconstructed.offset(), 4096 + 128);
    }

    #[test]
    fn test_object_header_flags() {
        let mut header = ObjectHeader::new(0, Uuid::new_v4(), 100);

        assert!(!header.is_deleted());
        header.set_deleted(true);
        assert!(header.is_deleted());

        assert!(!header.has_overflow());
        header.set_overflow(true);
        assert!(header.has_overflow());

        assert!(!header.is_pinned());
        header.set_pinned(true);
        assert!(header.is_pinned());
    }

    #[test]
    fn test_vardata_encoding() {
        let data = HashMap::from([
            ("name".to_string(), "Alice".to_string()),
            ("age".to_string(), "30".to_string()),
        ]);

        let vardata = VarData::encode(&data).unwrap();
        assert!(vardata.size() > 0);

        let decoded: HashMap<String, String> = vardata.decode().unwrap();
        assert_eq!(decoded.get("name").unwrap(), "Alice");
        assert_eq!(decoded.get("age").unwrap(), "30");
    }

    #[test]
    fn test_object_creation() {
        let id = Uuid::new_v4();
        let fixed = vec![1, 2, 3, 4];
        let var = VarData::empty();
        let refs = vec![ObjRef::new(100), ObjRef::new(200)];

        let obj = Object::new(1, id, fixed.clone(), var, refs.clone());

        assert_eq!(obj.header.type_tag, 1);
        assert_eq!(obj.header.id, id);
        assert_eq!(obj.fixed, fixed);
        assert_eq!(obj.refs, refs);
    }

    #[test]
    fn test_object_serialization() {
        let id = Uuid::new_v4();
        let fixed = vec![1, 2, 3, 4];
        let var = VarData::encode(&"test data").unwrap();
        let refs = vec![ObjRef::new(100)];

        let obj = Object::new(1, id, fixed, var, refs);
        let bytes = obj.to_bytes().unwrap();

        assert!(bytes.len() >= std::mem::size_of::<ObjectHeader>());
    }

    #[test]
    fn test_object_checksum() {
        let id = Uuid::new_v4();
        let fixed = vec![1, 2, 3, 4];
        let var = VarData::empty();
        let refs = vec![];

        let mut obj = Object::new(1, id, fixed, var, refs);
        
        // Calculate checksum based on serialized data
        let bytes = obj.to_bytes().unwrap();
        obj.header.checksum = obj.header.calculate_checksum(&bytes);

        // Verify the checksum matches
        let bytes2 = obj.to_bytes().unwrap();
        assert!(obj.header.verify_checksum(&bytes2));
    }
}
