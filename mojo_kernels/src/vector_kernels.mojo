# Note: this is an optional acceleration module.
# Build a shared library exporting `owl_mojo_vector_topk_f32` and set:
# OWL_MOJO_KERNELS_LIB=/absolute/path/to/libowl_mojo_kernels.{dylib|so}

from python import Python

@export
fn owl_mojo_vector_topk_f32(
    vectors_ptr: UnsafePointer[Float32],
    vector_count: Int,
    dimension: Int,
    query_ptr: UnsafePointer[Float32],
    metric: UInt32,
    k: Int,
    out_indices: UnsafePointer[UInt32],
    out_scores: UnsafePointer[Float32],
    out_count: UnsafePointer[Int],
) -> Int32:
    if vector_count <= 0 or dimension <= 0 or k <= 0:
        out_count.store(0)
        return 0

    let topk = k if k < vector_count else vector_count
    out_count.store(topk)

    # Baseline implementation (portable). Can be replaced with SIMD kernels.
    # Keeps semantics aligned with Zig fallback.
    var scores = Python.list()
    for row in range(vector_count):
        var dot: Float64 = 0.0
        var norm_a: Float64 = 0.0
        var norm_b: Float64 = 0.0
        var sq: Float64 = 0.0
        let base = row * dimension
        for i in range(dimension):
            let a = Float64(vectors_ptr[base + i])
            let b = Float64(query_ptr[i])
            dot = dot + (a * b)
            norm_a = norm_a + (a * a)
            norm_b = norm_b + (b * b)
            let d = a - b
            sq = sq + (d * d)

        var score: Float32 = 0.0
        if metric == 0:
            if norm_a > 0.0 and norm_b > 0.0:
                score = Float32(dot / (Python.sqrt(norm_a * norm_b)))
        elif metric == 1:
            score = Float32(-Python.sqrt(sq))
        else:
            score = Float32(dot)

        scores.append((score, row))

    scores.sort(reverse=True)
    for i in range(topk):
        let item = scores[i]
        out_scores[i] = item[0]
        out_indices[i] = UInt32(item[1])

    return 0
