use serde::{Deserialize, Serialize};
use std::fmt;

/// Stable, generation-checked identity for an object.
///
/// The packed representation is deliberately independent of a file location:
///
/// ```text
///  63             48 47                              16 15              0
/// +-----------------+----------------------------------+------------------+
/// | collection (16) |          object id (32)          | generation (16)  |
/// +-----------------+----------------------------------+------------------+
/// ```
///
/// Zero is reserved for the null reference. Physical record locations are
/// resolved through the object directory and may change during compaction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[repr(transparent)]
pub struct ObjRef(u64);

impl ObjRef {
    const COLLECTION_SHIFT: u32 = 48;
    const OBJECT_SHIFT: u32 = 16;

    /// Construct from the packed representation.
    ///
    /// Kept for format decoding and compatibility at API boundaries. New
    /// storage code should normally call [`ObjRef::from_parts`].
    pub const fn new(packed: u64) -> Self {
        Self(packed)
    }

    pub const fn from_parts(collection_id: u16, object_id: u32, generation: u16) -> Self {
        Self(
            ((collection_id as u64) << Self::COLLECTION_SHIFT)
                | ((object_id as u64) << Self::OBJECT_SHIFT)
                | generation as u64,
        )
    }

    pub const fn null() -> Self {
        Self(0)
    }

    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    pub const fn collection_id(self) -> u16 {
        (self.0 >> Self::COLLECTION_SHIFT) as u16
    }

    pub const fn object_id(self) -> u32 {
        (self.0 >> Self::OBJECT_SHIFT) as u32
    }

    pub const fn generation(self) -> u16 {
        self.0 as u16
    }

    pub const fn packed(self) -> u64 {
        self.0
    }

    /// Compatibility name for callers that formerly treated references as
    /// offsets. It returns the packed identity, never a physical file offset.
    pub const fn offset(self) -> u64 {
        self.packed()
    }

    pub fn from_string(value: &str) -> Option<Self> {
        if value.is_empty() {
            return Some(Self::null());
        }
        let encoded = value.strip_prefix("ref_")?;
        u64::from_str_radix(encoded, 16).ok().map(Self)
    }

    pub fn to_string_id(self) -> String {
        if self.is_null() {
            String::new()
        } else {
            format!("ref_{:016x}", self.0)
        }
    }
}

impl fmt::Debug for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            return f.write_str("ObjRef(null)");
        }
        f.debug_struct("ObjRef")
            .field("collection", &self.collection_id())
            .field("object", &self.object_id())
            .field("generation", &self.generation())
            .finish()
    }
}

impl fmt::Display for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_string_id())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_round_trip_without_encoding_a_location() {
        let reference = ObjRef::from_parts(42, 7_000_000, 3);
        assert_eq!(reference.collection_id(), 42);
        assert_eq!(reference.object_id(), 7_000_000);
        assert_eq!(reference.generation(), 3);
        assert_eq!(
            ObjRef::from_string(&reference.to_string_id()),
            Some(reference)
        );
    }

    #[test]
    fn null_is_reserved() {
        assert!(ObjRef::null().is_null());
        assert_eq!(ObjRef::null().to_string_id(), "");
    }
}
