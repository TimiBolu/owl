# Owl Storage Engine (Zig Implementation)

Low-level storage primitives for OQL database, focusing on O(1) object graph navigation through physical pointers.

## Project Status

**Phase 1: Core Types & Memory Management** ✅ COMPLETE

- [x] Task 1.1: ObjRef Implementation ✅
- [x] Task 1.2: ObjectHeader Implementation ✅
- [x] Task 1.3: Object Structure ✅
- [x] Task 1.4: Arena Allocator - Basic ✅
- [x] Task 1.5: Arena Allocator - Co-location ✅

**Phase 2: C API & Rust Integration** (In Progress)

- [x] Task 2.1: C API - Core Operations ✅
- [ ] Task 2.2: Rust Bindings - Safe Wrappers
- [ ] Task 2.3: Rust Integration - Query Layer Connection

**Overall Progress**: 6/27 tasks complete (22%)

See `.kiro/specs/zig-storage-rewrite/tasks.md` for full task list.

## Building

Requires Zig 0.12.0 or later.

```bash
# Build library
zig build

# Run tests
zig build test

# Run benchmarks
zig build bench
# or use the convenience script:
./run_benchmarks.sh
```

## Benchmarks

Comprehensive benchmark suite covering:
- **Allocation**: Sequential, mixed sizes, large objects
- **Navigation**: Sequential, random in-page, random across pages
- **Co-location**: Success and failure cases
- **Access Patterns**: Sequential, random, hot spots (80/20 rule)
- **Worst Cases**: Fragmentation, page thrashing
- **Memory Efficiency**: Overhead ratio, density

Run with: `zig build bench` or `./run_benchmarks.sh`

## Project Structure

```
owl_storage_zig/
├── build.zig              # Build configuration
├── src/
│   ├── objref.zig        # Task 1.1: 64-bit object references ✅
│   ├── object_header.zig # Task 1.2: Object headers (TODO)
│   ├── object.zig        # Task 1.3: Complete objects (TODO)
│   ├── arena.zig         # Task 1.4-1.5: Arena allocator (TODO)
│   ├── c_api.zig         # Task 2.1: C API exports (TODO)
│   └── test_all.zig      # Test runner
├── benches/
│   └── bench_all.zig     # Benchmarks (TODO)
└── README.md
```

## Design Goals

1. **O(1) Navigation**: Object references are physical pointers that can be swizzled in <10ns
2. **Cache Efficiency**: Co-location algorithm keeps related objects in same 4KB page
3. **Zero-Copy**: Direct memory access through mmap, no serialization overhead
4. **C ABI Compatible**: Clean FFI for Rust integration
5. **Explicit Control**: No hidden allocations, predictable performance

## Scope

This storage engine provides **low-level primitives only**:
- Physical object storage (ObjRef, ObjectHeader, Object)
- Memory-mapped arena allocation
- O(1) pointer swizzling and navigation
- Write-ahead logging for durability
- MVCC transactions

**Out of scope** (handled by Rust query layer):
- Collections and document abstractions
- Schema management and validation
- Query planning and optimization
- Index management
- High-level API (OQL syntax)

## Performance Targets

| Metric | Target | Status |
|--------|--------|--------|
| Navigation | <50ns per hop | ⏳ |
| Allocation | <20ns per object | ⏳ |
| Memory overhead | <1.2x object size | ⏳ |
| Write throughput | >1M TPS | ⏳ |
| Read throughput | >500k QPS | ⏳ |

## License

Same as parent project.
