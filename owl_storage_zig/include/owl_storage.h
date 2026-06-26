#ifndef OWL_STORAGE_H
#define OWL_STORAGE_H

#include <stdint.h>
#include <stddef.h>
#include <stdbool.h>

#ifdef __cplusplus
extern "C" {
#endif

// Error codes
#define OWL_OK                  0
#define OWL_ERR_OUT_OF_MEMORY  -1
#define OWL_ERR_INVALID_ARENA  -2
#define OWL_ERR_INVALID_REF    -3
#define OWL_ERR_DATA_TOO_SMALL -4
#define OWL_ERR_OUT_OF_BOUNDS  -5
#define OWL_ERR_OBJECT_DELETED -6

// Opaque arena handle
typedef struct Arena Arena;

/**
 * Create a new arena
 * @param path Path to the backing file
 * @param size Size in bytes
 * @return Arena handle or NULL on failure
 */
Arena* owl_arena_create(const char* path, size_t size);

/**
 * Destroy an arena and free all resources
 * @param arena Arena handle
 */
void owl_arena_destroy(Arena* arena);

/**
 * Allocate an object in the arena
 * @param arena Arena handle
 * @param size Size of object (including header)
 * @param out_ref Output parameter for object reference
 * @return OWL_OK on success, error code otherwise
 */
int owl_arena_alloc(Arena* arena, uint32_t size, uint64_t* out_ref);

/**
 * Allocate an object near another object (for co-location)
 * @param arena Arena handle
 * @param size Size of object (including header)
 * @param near Reference to allocate near (0 for no preference)
 * @param out_ref Output parameter for object reference
 * @return OWL_OK on success, error code otherwise
 */
int owl_arena_alloc_near(Arena* arena, uint32_t size, uint64_t near, uint64_t* out_ref);

/**
 * Write data to an object
 * @param arena Arena handle
 * @param obj_ref Object reference
 * @param data Data to write
 * @param data_len Length of data
 * @return OWL_OK on success, error code otherwise
 */
int owl_arena_write(Arena* arena, uint64_t obj_ref, const uint8_t* data, size_t data_len);

/**
 * Read an object from the arena
 * @param arena Arena handle
 * @param obj_ref Object reference
 * @param buffer Buffer to read into
 * @param buffer_len Size of buffer
 * @param out_bytes_read Output parameter for bytes read
 * @return OWL_OK on success, error code otherwise
 */
int owl_arena_read(Arena* arena, uint64_t obj_ref, uint8_t* buffer, size_t buffer_len, size_t* out_bytes_read);

/**
 * Delete an object
 * @param arena Arena handle
 * @param obj_ref Object reference
 * @return OWL_OK on success, error code otherwise
 */
int owl_arena_delete(Arena* arena, uint64_t obj_ref);

/**
 * Get arena statistics
 * @param arena Arena handle
 * @param out_total Output parameter for total size
 * @param out_used Output parameter for used size
 * @param out_free Output parameter for free size
 * @param out_object_count Output parameter for object count
 * @return OWL_OK on success, error code otherwise
 */
int owl_arena_stats(Arena* arena, size_t* out_total, size_t* out_used, size_t* out_free, uint64_t* out_object_count);

/**
 * Get co-location metrics
 * @param arena Arena handle
 * @param out_total_attempts Output parameter for total attempts
 * @param out_successful Output parameter for successful co-locations
 * @param out_failed Output parameter for failed co-locations
 * @param out_success_rate Output parameter for success rate (percentage)
 * @return OWL_OK on success, error code otherwise
 */
int owl_arena_colocation_metrics(Arena* arena, size_t* out_total_attempts, size_t* out_successful, size_t* out_failed, double* out_success_rate);

/**
 * Get the base pointer for swizzling (advanced usage)
 * @param arena Arena handle
 * @return Base pointer or NULL on failure
 */
uint8_t* owl_arena_base_ptr(Arena* arena);

/**
 * Recover best superblock record.
 * @return 1 if a record was found, 0 if not found, negative error code on failure
 */
int owl_superblock_recover(
    const char* path,
    bool strict,
    uint64_t* out_sequence,
    uint64_t* out_arena_size,
    uint64_t* out_wal_size,
    uint64_t* out_document_count,
    uint32_t* out_schema_count,
    uint32_t* out_record_crc32
);

/**
 * Persist superblock copyset record.
 * @return OWL_OK on success, negative error code otherwise
 */
int owl_superblock_persist(
    const char* path,
    uint64_t sequence,
    uint32_t parent_crc32,
    uint64_t arena_size,
    uint64_t wal_size,
    uint64_t document_count,
    uint32_t schema_count,
    bool initial_full,
    uint32_t* out_record_crc32
);

/**
 * Append framed WAL record (magic + metadata + CRC-protected payload).
 * @return OWL_OK on success, negative error code otherwise
 */
int owl_wal_append_framed(
    const char* path,
    const uint8_t* payload,
    size_t payload_len
);

/**
 * Exact top-k vector search over dense row-major f32 vectors.
 *
 * Metric encoding:
 * - 0: cosine similarity
 * - 1: negative L2 distance
 * - 2: dot product
 *
 * @return OWL_OK on success, negative error code otherwise
 */
int owl_vector_topk_f32(
    const float* vectors_ptr,
    size_t vector_count,
    size_t dimension,
    const float* query_ptr,
    uint32_t metric,
    size_t k,
    uint32_t* out_indices,
    float* out_scores,
    size_t* out_count
);

/**
 * Create a new HNSW index handle.
 * Returns NULL on failure.
 */
void* owl_hnsw_index_create(
    uint32_t dimension,
    uint32_t m,
    uint32_t ef_construction
);

/**
 * Destroy an HNSW index handle.
 */
void owl_hnsw_index_destroy(void* index);

/**
 * Add a vector to HNSW index.
 * @return OWL_OK on success, negative error code otherwise
 */
int owl_hnsw_index_add_vector(
    void* index,
    uint64_t id,
    const float* vector_ptr,
    size_t vector_len
);

/**
 * Search HNSW index.
 * @return OWL_OK on success, negative error code otherwise
 */
int owl_hnsw_index_search(
    void* index,
    const float* query_ptr,
    size_t query_len,
    size_t k,
    size_t ef,
    uint64_t* out_ids,
    float* out_scores,
    size_t* out_count
);

/**
 * Save HNSW index to disk.
 * @return OWL_OK on success, negative error code otherwise
 */
int owl_hnsw_index_save(void* index, const char* path);

/**
 * Load HNSW index from disk.
 * Returns NULL on failure.
 */
void* owl_hnsw_index_load(const char* path);

#ifdef __cplusplus
}
#endif

#endif // OWL_STORAGE_H
