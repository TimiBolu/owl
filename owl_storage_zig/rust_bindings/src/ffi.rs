//! FFI declarations for Owl Storage C API

use std::os::raw::c_char;

/// Opaque arena handle
#[repr(C)]
pub struct Arena {
    _private: [u8; 0],
}

extern "C" {
    pub fn owl_arena_create(path: *const c_char, size: usize) -> *mut Arena;
    pub fn owl_arena_destroy(arena: *mut Arena);
    pub fn owl_arena_alloc(arena: *mut Arena, size: u32, out_ref: *mut u64) -> i32;
    pub fn owl_arena_alloc_near(
        arena: *mut Arena,
        size: u32,
        near: u64,
        out_ref: *mut u64,
    ) -> i32;
    pub fn owl_arena_write(
        arena: *mut Arena,
        obj_ref: u64,
        data: *const u8,
        data_len: usize,
    ) -> i32;
    pub fn owl_arena_read(
        arena: *mut Arena,
        obj_ref: u64,
        buffer: *mut u8,
        buffer_len: usize,
        out_bytes_read: *mut usize,
    ) -> i32;
    pub fn owl_arena_delete(arena: *mut Arena, obj_ref: u64) -> i32;
    pub fn owl_arena_stats(
        arena: *mut Arena,
        out_total: *mut usize,
        out_used: *mut usize,
        out_free: *mut usize,
        out_object_count: *mut u64,
    ) -> i32;
    pub fn owl_arena_colocation_metrics(
        arena: *mut Arena,
        out_total_attempts: *mut usize,
        out_successful: *mut usize,
        out_failed: *mut usize,
        out_success_rate: *mut f64,
    ) -> i32;
    pub fn owl_arena_base_ptr(arena: *mut Arena) -> *mut u8;
}
