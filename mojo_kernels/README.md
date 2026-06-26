# Owl Mojo Kernels (Optional)

This directory contains optional Mojo acceleration kernels for Owl vector search.

## Runtime contract

Rust bindings (`owl_storage_zig/rust_bindings/src/vector.rs`) can load a shared library at runtime when:

- env var `OWL_MOJO_KERNELS_LIB` is set to a valid shared library path
- symbol `owl_mojo_vector_topk_f32` is exported

If loading fails, Owl automatically falls back to the Zig implementation (`owl_vector_topk_f32`).

## ABI

See:

- `mojo_kernels/include/owl_mojo_kernels.h`

The Mojo implementation must return Owl-compatible error codes (`0` success, negative on error).

## Notes

- Mojo integration is optional by design.
- Production correctness does not depend on Mojo being present.
- Keep metric semantics aligned with Zig:
  - `0`: cosine
  - `1`: negative L2
  - `2`: dot product
