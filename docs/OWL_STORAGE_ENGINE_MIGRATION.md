# Historical storage migration note

This document previously described an intermediate Rust wrapper over Owl's Zig
storage engine. That architecture no longer exists.

Owl now has one entirely Rust-owned storage implementation under
`src/storage_engine/`. There is no native storage dependency or FFI correctness
boundary. The canonical design and implementation sequence are documented in
[Storage Engine Architecture](STORAGE_ENGINE_ARCHITECTURE.md).

The current Rust WAL format is version 2 and embeds `OWLR` record format version
1. Checkpoints use `OWLS` segment format version 2, alternating `OWLM` manifest
format version 2 sidecars, manifest-referenced cumulative paged `OWLD` format 2
record roots (with format 1 read compatibility), and cumulative `OWLI` scalar
ordered-index roots. Manifest decoding remains compatible with format 1. No
migration utility from the removed prototype or the short-lived Rust WAL version
1 is provided; existing prototype databases should be treated as incompatible
development data.
