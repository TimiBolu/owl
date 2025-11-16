#ifndef OWL_STORAGE_H
#define OWL_STORAGE_H

#include <stdint.h>
#include <stddef.h>

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

#ifdef __cplusplus
}
#endif

#endif // OWL_STORAGE_H
