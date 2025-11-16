//! Object reference type

use std::fmt;

/// A 64-bit physical pointer to an object in the arena.
///
/// ObjRef represents a file-global offset that can be converted to a memory
/// pointer in O(1) time using swizzling. This is the core abstraction that
/// enables fast graph navigation.
///
/// # Memory Layout
///
/// - offset: u64 (8 bytes)
///
/// # Page and Slot Calculation
///
/// - Page size: 4KB (4096 bytes)
/// - Slot size: 64 bytes
/// - Page ID: offset / 4096
/// - Slot ID: (offset % 4096) / 64
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct ObjRef(u64);

impl ObjRef {
    /// Create a new object reference from an offset
    #[inline]
    pub const fn new(offset: u64) -> Self {
        Self(offset)
    }

    /// Create a null reference (offset = 0)
    #[inline]
    pub const fn null() -> Self {
        Self(0)
    }

    /// Check if this is a null reference
    #[inline]
    pub const fn is_null(self) -> bool {
        self.0 == 0
    }

    /// Get the raw offset value
    #[inline]
    pub const fn offset(self) -> u64 {
        self.0
    }

    /// Get the page ID (4KB page number) for this reference
    #[inline]
    pub const fn page_id(self) -> u32 {
        (self.0 / 4096) as u32
    }

    /// Get the slot ID within the page (64-byte slots)
    #[inline]
    pub const fn slot_id(self) -> u16 {
        ((self.0 % 4096) / 64) as u16
    }

    /// Create an ObjRef from page_id and slot_id
    #[inline]
    pub const fn from_page_slot(page_id: u32, slot_id: u16) -> Self {
        let offset = (page_id as u64 * 4096) + (slot_id as u64 * 64);
        Self(offset)
    }

    /// Swizzle: Convert disk offset to memory pointer
    ///
    /// # Safety
    ///
    /// The caller must ensure:
    /// - base pointer is valid
    /// - offset is within bounds
    /// - resulting pointer points to valid Object data
    #[inline]
    pub unsafe fn swizzle(self, base: *const u8) -> *const u8 {
        base.add(self.0 as usize)
    }

    /// Swizzle to mutable pointer
    ///
    /// # Safety
    ///
    /// Same safety requirements as `swizzle`, plus:
    /// - No other references to this object exist
    #[inline]
    pub unsafe fn swizzle_mut(self, base: *mut u8) -> *mut u8 {
        base.add(self.0 as usize)
    }
}

impl fmt::Display for ObjRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_null() {
            write!(f, "ObjRef(null)")
        } else {
            write!(
                f,
                "ObjRef(offset={}, page={}, slot={})",
                self.offset(),
                self.page_id(),
                self.slot_id()
            )
        }
    }
}

impl From<u64> for ObjRef {
    #[inline]
    fn from(offset: u64) -> Self {
        Self(offset)
    }
}

impl From<ObjRef> for u64 {
    #[inline]
    fn from(objref: ObjRef) -> Self {
        objref.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_null_reference() {
        let null_ref = ObjRef::null();
        assert!(null_ref.is_null());
        assert_eq!(null_ref.offset(), 0);
    }

    #[test]
    fn test_non_null_reference() {
        let ref1 = ObjRef::new(1024);
        assert!(!ref1.is_null());
        assert_eq!(ref1.offset(), 1024);
    }

    #[test]
    fn test_page_and_slot() {
        // Page 0, slot 0
        let ref0 = ObjRef::new(0);
        assert_eq!(ref0.page_id(), 0);
        assert_eq!(ref0.slot_id(), 0);

        // Page 1, slot 2 (offset = 4096 + 128)
        let ref1 = ObjRef::new(4096 + 128);
        assert_eq!(ref1.page_id(), 1);
        assert_eq!(ref1.slot_id(), 2);

        // Last slot of page 0
        let ref_last = ObjRef::new(4096 - 64);
        assert_eq!(ref_last.page_id(), 0);
        assert_eq!(ref_last.slot_id(), 63);
    }

    #[test]
    fn test_from_page_slot() {
        let page_id = 5;
        let slot_id = 10;

        let objref = ObjRef::from_page_slot(page_id, slot_id);
        assert_eq!(objref.page_id(), page_id);
        assert_eq!(objref.slot_id(), slot_id);

        let expected_offset = (page_id as u64 * 4096) + (slot_id as u64 * 64);
        assert_eq!(objref.offset(), expected_offset);
    }

    #[test]
    fn test_display() {
        let null_ref = ObjRef::null();
        assert_eq!(format!("{}", null_ref), "ObjRef(null)");

        let ref1 = ObjRef::new(4096 + 128);
        assert!(format!("{}", ref1).contains("offset=4224"));
        assert!(format!("{}", ref1).contains("page=1"));
        assert!(format!("{}", ref1).contains("slot=2"));
    }

    #[test]
    fn test_conversions() {
        let offset: u64 = 1024;
        let objref = ObjRef::from(offset);
        assert_eq!(objref.offset(), offset);

        let back: u64 = objref.into();
        assert_eq!(back, offset);
    }
}
