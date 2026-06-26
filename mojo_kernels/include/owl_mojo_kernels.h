#ifndef OWL_MOJO_KERNELS_H
#define OWL_MOJO_KERNELS_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

// Metric encoding:
// 0 = cosine, 1 = negative L2, 2 = dot
int owl_mojo_vector_topk_f32(
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

#ifdef __cplusplus
}
#endif

#endif
