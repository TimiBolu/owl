//! FFI declarations for Owl Storage C API

use std::os::raw::c_char;

/// Opaque arena handle
#[repr(C)]
pub struct Arena {
    _private: [u8; 0],
}

/// Opaque B-tree handle
#[repr(C)]
pub struct BTree {
    _private: [u8; 0],
}

/// Opaque HNSW index handle
#[repr(C)]
pub struct HnswIndex {
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
    pub fn owl_arena_write_at(
        arena: *mut Arena,
        offset: u64,
        data: *const u8,
        data_len: usize,
    ) -> i32;
    pub fn owl_arena_read_at(
        arena: *mut Arena,
        offset: u64,
        buffer: *mut u8,
        buffer_len: usize,
        out_bytes_read: *mut usize,
    ) -> i32;
    pub fn owl_superblock_recover(
        path: *const c_char,
        strict: bool,
        out_sequence: *mut u64,
        out_arena_size: *mut u64,
        out_wal_size: *mut u64,
        out_document_count: *mut u64,
        out_schema_count: *mut u32,
        out_record_crc32: *mut u32,
    ) -> i32;
    pub fn owl_superblock_persist(
        path: *const c_char,
        sequence: u64,
        parent_crc32: u32,
        arena_size: u64,
        wal_size: u64,
        document_count: u64,
        schema_count: u32,
        initial_full: bool,
        out_record_crc32: *mut u32,
    ) -> i32;
    pub fn owl_wal_append_framed(path: *const c_char, payload: *const u8, payload_len: usize) -> i32;

    pub fn owl_btree_create(order: u32, shard_id: u32) -> *mut BTree;
    pub fn owl_btree_destroy(btree: *mut BTree);
    pub fn owl_btree_insert(btree: *mut BTree, key: u64, value_offset: u64) -> i32;
    pub fn owl_btree_search(
        btree: *mut BTree,
        key: u64,
        out_value_offset: *mut u64,
        out_found: *mut bool,
    ) -> i32;
    pub fn owl_btree_delete(btree: *mut BTree, key: u64, out_deleted: *mut bool) -> i32;
    pub fn owl_btree_range_query(
        btree: *mut BTree,
        start_key: u64,
        end_key: u64,
        out_value_offsets: *mut u64,
        capacity: usize,
        out_count: *mut usize,
    ) -> i32;

    pub fn owl_vector_topk_f32(
        vectors_ptr: *const f32,
        vector_count: usize,
        dimension: usize,
        query_ptr: *const f32,
        metric: u32,
        k: usize,
        out_indices: *mut u32,
        out_scores: *mut f32,
        out_count: *mut usize,
    ) -> i32;

    pub fn owl_hnsw_index_create(dimension: u32, m: u32, ef_construction: u32) -> *mut HnswIndex;
    pub fn owl_hnsw_index_destroy(index: *mut HnswIndex);
    pub fn owl_hnsw_index_add_vector(
        index: *mut HnswIndex,
        id: u64,
        vector_ptr: *const f32,
        vector_len: usize,
    ) -> i32;
    pub fn owl_hnsw_index_search(
        index: *mut HnswIndex,
        query_ptr: *const f32,
        query_len: usize,
        k: usize,
        ef: usize,
        out_ids: *mut u64,
        out_scores: *mut f32,
        out_count: *mut usize,
    ) -> i32;
    pub fn owl_hnsw_index_save(index: *mut HnswIndex, path: *const c_char) -> i32;
    pub fn owl_hnsw_index_load(path: *const c_char) -> *mut HnswIndex;
}
