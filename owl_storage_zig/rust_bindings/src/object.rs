//! Object wrapper for creating and managing storage objects

use crate::error::{Error, Result};
use crate::objref::ObjRef;
use std::mem;

/// Object header size in bytes
pub const OBJECT_HEADER_SIZE: usize = 56;

/// A storage object with header, fixed data, variable data, and references.
///
/// Objects are the fundamental unit of storage in Owl. Each object has:
/// - A 56-byte header with metadata
/// - Fixed-size data (specified at creation)
/// - Variable-size data (optional)
/// - References to other objects (optional)
pub struct Object {
    /// Type tag for the object (u16 to match Zig)
    pub type_tag: u16,
    /// Object ID (16 bytes)
    pub id: [u8; 16],
    /// Fixed-size data
    pub fixed: Vec<u8>,
    /// Variable-size data
    pub var_data: Vec<u8>,
    /// References to other objects
    pub refs: Vec<ObjRef>,
}

impl Object {
    /// Create a new object.
    ///
    /// # Arguments
    ///
    /// * `type_tag` - Type identifier for the object (u16)
    /// * `id` - Unique 16-byte identifier
    /// * `fixed` - Fixed-size data
    /// * `var_data` - Variable-size data
    /// * `refs` - References to other objects
    ///
    /// # Example
    ///
    /// ```
    /// use owl_storage_zig::Object;
    ///
    /// let id = [1u8; 16];
    /// let fixed = vec![0xAA; 64];
    /// let obj = Object::new(1, id, fixed, vec![], vec![]);
    /// ```
    pub fn new(
        type_tag: u16,
        id: [u8; 16],
        fixed: Vec<u8>,
        var_data: Vec<u8>,
        refs: Vec<ObjRef>,
    ) -> Self {
        Self {
            type_tag,
            id,
            fixed,
            var_data,
            refs,
        }
    }

    /// Serialize the object to bytes for storage.
    ///
    /// The serialized format matches Zig exactly:
    /// - Header (56 bytes)
    /// - fixed_len (4 bytes, u32 little-endian)
    /// - var_len (4 bytes, u32 little-endian)
    /// - refs_count (4 bytes, u32 little-endian)
    /// - Fixed data
    /// - Variable data
    /// - References (8 bytes each)
    ///
    /// # Returns
    ///
    /// Returns a byte vector containing the serialized object.
    pub fn to_bytes(&self) -> Vec<u8> {
        let total_size = self.total_size();
        let mut bytes = Vec::with_capacity(total_size);

        // Create header
        let header = ObjectHeader::new(self.type_tag, self.id, total_size as u32);

        // Serialize header
        bytes.extend_from_slice(&header.to_bytes());

        // Write lengths (3 x u32 little-endian)
        bytes.extend_from_slice(&(self.fixed.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.var_data.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(self.refs.len() as u32).to_le_bytes());

        // Serialize fixed data
        bytes.extend_from_slice(&self.fixed);

        // Serialize variable data
        bytes.extend_from_slice(&self.var_data);

        // Serialize references
        for objref in &self.refs {
            bytes.extend_from_slice(&objref.offset().to_le_bytes());
        }

        bytes
    }

    /// Deserialize an object from bytes.
    ///
    /// # Arguments
    ///
    /// * `bytes` - Serialized object data
    ///
    /// # Errors
    ///
    /// Returns an error if:
    /// - Data is too small for header
    /// - Data size doesn't match header
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < OBJECT_HEADER_SIZE {
            return Err(Error::DataTooSmall);
        }

        // Parse header
        let header = ObjectHeader::from_bytes(&bytes[..OBJECT_HEADER_SIZE])?;

        // Validate total size
        if bytes.len() < header.size as usize {
            return Err(Error::DataTooSmall);
        }

        let mut offset = OBJECT_HEADER_SIZE;

        // Read lengths (3 x u32 little-endian)
        if bytes.len() < offset + 12 {
            return Err(Error::DataTooSmall);
        }

        let fixed_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let var_len = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;
        let refs_count = u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
        offset += 4;

        // Parse fixed data
        if bytes.len() < offset + fixed_len {
            return Err(Error::DataTooSmall);
        }
        let fixed = bytes[offset..offset + fixed_len].to_vec();
        offset += fixed_len;

        // Parse variable data
        if bytes.len() < offset + var_len {
            return Err(Error::DataTooSmall);
        }
        let var_data = bytes[offset..offset + var_len].to_vec();
        offset += var_len;

        // Parse references
        let refs_size = refs_count * 8;
        if bytes.len() < offset + refs_size {
            return Err(Error::DataTooSmall);
        }

        let mut refs = Vec::with_capacity(refs_count);
        for i in 0..refs_count {
            let ref_offset = offset + i * 8;
            let ref_bytes = &bytes[ref_offset..ref_offset + 8];
            let offset_val = u64::from_le_bytes(ref_bytes.try_into().unwrap());
            refs.push(ObjRef::new(offset_val));
        }

        Ok(Object {
            type_tag: header.type_tag,
            id: header.id,
            fixed,
            var_data,
            refs,
        })
    }

    /// Calculate the total size of the serialized object.
    pub fn total_size(&self) -> usize {
        OBJECT_HEADER_SIZE + 12 + self.fixed.len() + self.var_data.len() + (self.refs.len() * 8)
    }
}

/// Object header (56 bytes) - must match Zig layout exactly
#[repr(C)]
#[derive(Copy, Clone)]
struct ObjectHeader {
    type_tag: u16,      // 2 bytes
    id: [u8; 16],       // 16 bytes
    version: u32,       // 4 bytes
    size: u32,          // 4 bytes
    flags: u8,          // 1 byte
    _padding1: [u8; 7], // 7 bytes
    checksum: u32,      // 4 bytes
    _padding2: [u8; 4], // 4 bytes
    lsn: u64,           // 8 bytes
    // Total: 56 bytes
}

impl ObjectHeader {
    fn new(type_tag: u16, id: [u8; 16], size: u32) -> Self {
        Self {
            type_tag,
            id,
            version: 0,
            size,
            flags: 0,
            _padding1: [0; 7],
            checksum: 0,
            _padding2: [0; 4],
            lsn: 0,
        }
    }

    fn to_bytes(&self) -> [u8; OBJECT_HEADER_SIZE] {
        unsafe { mem::transmute(*self) }
    }

    fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() < OBJECT_HEADER_SIZE {
            return Err(Error::DataTooSmall);
        }
        let mut header_bytes = [0u8; OBJECT_HEADER_SIZE];
        header_bytes.copy_from_slice(&bytes[..OBJECT_HEADER_SIZE]);
        Ok(unsafe { mem::transmute(header_bytes) })
    }


}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_object_header_size() {
        assert_eq!(mem::size_of::<ObjectHeader>(), OBJECT_HEADER_SIZE);
    }

    #[test]
    fn test_object_serialization() {
        let id = [1u8; 16];
        let fixed = vec![0xAA; 64];
        let var_data = vec![0xBB; 32];
        let refs = vec![ObjRef::new(1024), ObjRef::new(2048)];

        let obj = Object::new(1, id, fixed.clone(), var_data.clone(), refs.clone());
        let bytes = obj.to_bytes();

        let obj2 = Object::from_bytes(&bytes).expect("Failed to deserialize");

        assert_eq!(obj2.type_tag, 1);
        assert_eq!(obj2.id, id);
        assert_eq!(obj2.fixed, fixed);
        assert_eq!(obj2.var_data, var_data);
        assert_eq!(obj2.refs.len(), 2);
        assert_eq!(obj2.refs[0].offset(), 1024);
        assert_eq!(obj2.refs[1].offset(), 2048);
    }

    #[test]
    fn test_object_total_size() {
        let id = [1u8; 16];
        let fixed = vec![0xAA; 64];
        let var_data = vec![0xBB; 32];
        let refs = vec![ObjRef::new(1024)];

        let obj = Object::new(1, id, fixed, var_data, refs);
        // Header (56) + 3 length fields (12) + fixed (64) + var (32) + refs (8)
        let expected_size = OBJECT_HEADER_SIZE + 12 + 64 + 32 + 8;
        assert_eq!(obj.total_size(), expected_size);
    }
}
