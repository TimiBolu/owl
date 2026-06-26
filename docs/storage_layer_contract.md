# Storage Layer Contract (Rust + Zig)

## Purpose
This contract defines the ownership boundary between Rust and Zig storage code so durability and recovery logic are not duplicated.

## Layer Ownership
- Zig owns low-level storage primitives: arena allocation, raw reads/writes, mmap backing, and low-level durability primitives.
- Rust owns high-level semantics: document framing (`[u32 len][u32 crc][payload]`), schema semantics, object graph lookups, and orchestration.
- Rust must not reimplement allocator internals or page management.
- Zig must not own Rust document/schema semantics.

## Durability Contract
- All mutating Rust storage operations use WAL-first ordering: reserve address -> append+sync WAL record -> apply arena mutation -> acknowledge.
- WAL records are append-only and length-prefixed.
- Recovery replays WAL records to rebuild in-memory metadata (`(schema,id)->objref`) and schema registry.

## Recovery Contract
- On startup, Rust replays storage WAL sidecar entries in log order.
- Replay is idempotent:
  - `Put` writes payload to recorded offset and updates the index map.
  - `Delete` removes index entries.
  - `PutSchema` updates schema registry.
- Rust distributed WAL rebuilds next LSN by scanning existing WAL file and rehydrates active transaction tracking.

## Invariants
- If `put_document` returns success, corresponding WAL entry was synced before arena payload apply.
- After restart, successful `put_document` data is readable by `(schema,id)` lookup.
- After restart, successful `put_schema` data is listed and retrievable.
- After restart, successful delete remains deleted.
