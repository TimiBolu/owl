# OQL Documentation

Welcome to the OQL database documentation!

## Quick Links

### Getting Started
- [Main README](../README.md) - Project overview and quick start
- [OQL Syntax](../owl_design_and_architecture/OQL_SYNTAX.md) - Query language reference
- [Architecture](../owl_design_and_architecture/OQL_ARCHITECTURE.md) - System architecture

### Storage Engine
- **[Zig Storage Design](../.kiro/specs/zig-storage-rewrite/design.md)** - Current storage engine design
- **[Implementation Tasks](../.kiro/specs/zig-storage-rewrite/tasks.md)** - Development progress
- **[Performance Benchmarks](../owl_storage_zig/BENCHMARKS.md)** - Performance metrics

### Important Notices

⚠️ **[RocksDB Deprecation Notice](./ROCKSDB_DEPRECATION.md)** - **READ THIS FIRST**

RocksDB storage has been deprecated and replaced with Zig Arena Storage. If you have existing RocksDB data or are upgrading from an older version, please read the deprecation notice for migration instructions.

## Documentation Structure

```
docs/
├── README.md                    # This file
├── ROCKSDB_DEPRECATION.md      # ⚠️ Important migration notice
└── (future documentation)

owl_design_and_architecture/
├── OQL_ARCHITECTURE.md         # System architecture
├── OQL_SYNTAX.md               # Query language syntax
├── OQL_PIPELINE_ANALYSIS.md    # Query pipeline analysis
└── TASKS_VERIFICATION.md       # Task verification

.kiro/specs/
├── zig-storage-rewrite/        # Zig storage engine spec
│   ├── requirements.md
│   ├── design.md
│   └── tasks.md
└── (other specs)

owl_storage_zig/
├── README.md                   # Zig storage implementation
├── BENCHMARKS.md               # Performance benchmarks
└── src/                        # Zig source code
```

## Key Concepts

### O(1) Navigation

OQL's core innovation is **O(1) pointer-based navigation**. When you write:

```oql
order.customer.profile.name
```

This executes as 2-3 pointer dereferences (~50ns), not database queries. This is possible because:

1. **ObjRef**: References are 64-bit file offsets, not strings
2. **Swizzling**: Offsets convert to memory pointers in O(1)
3. **Co-location**: Related objects stored in same 4KB page
4. **Memory-mapped**: Direct access to objects via mmap

### Storage Architecture

```
┌─────────────────────────────────────────────────────────┐
│   Rust: OQL Query Layer                                 │
│   - Parser, Executor, Schema                            │
└────────────────────┬────────────────────────────────────┘
                     │ C ABI
┌────────────────────▼────────────────────────────────────┐
│   Zig: Storage Engine                                    │
│   - Arena allocation, WAL, MVCC, Cache                  │
└────────────────────┬────────────────────────────────────┘
                     │ mmap
┌────────────────────▼────────────────────────────────────┐
│   Operating System                                       │
│   - Page cache, File system                             │
└─────────────────────────────────────────────────────────┘
```

### Performance Targets

| Metric | Target | Status |
|--------|--------|--------|
| Navigation | <50ns per hop | ✅ Achieved |
| Allocation | <20ns per object | ✅ Achieved |
| Memory Overhead | <1.2x | ✅ Achieved |
| Co-location | >70% | ✅ Achieved (79%) |
| Compile Time | <10s | ✅ Achieved |

## Development Status

### ✅ Completed Phases

- **Phase 1**: Core Types & Memory Management
- **Phase 2**: Storage Layer
- **Phase 3**: WAL & Durability
- **Phase 4**: Transactions & MVCC
- **Phase 5**: Performance & Optimization
- **Phase 8** (partial): Query Engine Integration

### 🚧 In Progress

- **Phase 6**: Testing & Validation
- **Phase 8**: Query Engine Integration (O(1) navigation working)

### ⏳ Upcoming

- **Phase 7**: Documentation & Polish
- **Phase 9**: Query Planning & Optimization
- **Phase 10**: Indexes & Advanced Features
- **Phase 11**: Final Integration & Testing

## Contributing

We welcome contributions! Areas where help is needed:

1. **Testing**: Property-based tests, crash recovery tests
2. **Documentation**: Examples, tutorials, API docs
3. **Performance**: Benchmarking, profiling, optimization
4. **Migration Tools**: RocksDB to Zig migration utilities

See the [main README](../README.md) for contribution guidelines.

## Support

- **Issues**: [GitHub Issues](https://github.com/your-org/oql/issues)
- **Discussions**: [GitHub Discussions](https://github.com/your-org/oql/discussions)
- **Email**: maintainers@oql-db.org

## License

See [LICENSE](../LICENSE) file for details.

---

*Last Updated: November 16, 2025*
