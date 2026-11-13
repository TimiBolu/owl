# Owl Architecture Gaps: Current State, Consequences, and Direction

This document captures a practical view of where Owl is today in its Rust and Zig architecture, what that buys us, what it costs us, and what we likely need next for the performance and reliability goals we care about.

It is not a criticism document. It is a clarity document.

The central observation is simple:

- Owl query, document, and schema semantics live primarily in Rust.
- Owl low-level storage allocation, byte persistence, and pointer substrate live primarily in Zig.
- The main document persistence path today is closer to:

```text
Rust Document -> serialized blob -> Zig arena
```

than to:

```text
Rust Document -> native Zig object graph -> query over native storage objects
```

That fact is neither inherently bad nor sufficient for Owl's long-term ambitions. It is a stage. This document tries to make that stage explicit.

## 1. Current Architecture, Plainly

At a high level, Owl currently splits responsibility as follows.

Rust owns:
- query language and parser
- document and value semantics
- schema semantics and validation
- query execution
- function execution
- document serialization and deserialization
- most high-level indexing coordination

Zig owns:
- arena allocation
- physical object reference substrate
- byte persistence
- WAL and lower-level durability primitives
- memory-mapped storage mechanics
- storage-oriented experimental primitives

In concrete terms, the executor hands a `Document` to `DocumentStore`, and the concrete `ZigStore` serializes a Rust-side `StoredDocument` payload and writes the resulting bytes into the Zig-backed arena.

Relevant code:
- Document model: `/Users/timiadesina/Documents/owl/src/document.rs:101`
- Query execution entry: `/Users/timiadesina/Documents/owl/src/executor.rs:179`
- Insert commit handoff: `/Users/timiadesina/Documents/owl/src/executor.rs:2067`
- Storage trait seam: `/Users/timiadesina/Documents/owl/src/storage.rs:41`
- ZigStore write entry: `/Users/timiadesina/Documents/owl/src/storage.rs:2198`
- Core blob write path: `/Users/timiadesina/Documents/owl/src/storage.rs:2038`
- Core blob read path: `/Users/timiadesina/Documents/owl/src/storage.rs:2070`
- Rust Arena wrapper over Zig: `/Users/timiadesina/Documents/owl/owl_storage_zig/rust_bindings/src/arena.rs:36`
- Zig FFI declarations: `/Users/timiadesina/Documents/owl/owl_storage_zig/rust_bindings/src/ffi.rs:26`

## 2. Why This Is Actually Good

This architecture has real strengths.

### 2.1 Clean responsibility split

The current split is easy to explain:

- Rust is the Owl runtime.
- Zig is the Owl storage substrate.

That is a strong separation of concerns. It prevents Owl language semantics from being prematurely embedded into low-level storage layout decisions.

### 2.2 Faster language evolution

Because the document model lives in Rust, Owl can still evolve:
- null semantics
- reference semantics
- query block semantics
- schema rules
- function semantics
- update semantics

without redesigning the physical storage model every time.

### 2.3 Lower implementation risk

A true native storage object graph tightly coupled to Owl semantics would be significantly harder to evolve safely. The current blob-oriented bridge reduces coupling while the language is still settling.

### 2.4 Zig remains focused

This is a particularly good property of the present design.

Zig is not being forced to become:
- a full semantic document interpreter
- a second query runtime
- a duplicate schema engine

before the system is ready.

That restraint is good engineering.

## 3. The Core Gap

The core gap is not that Zig is unused. Zig is very much used.

The gap is that the main document path uses Zig primarily as:
- allocator
- byte-addressed durable store
- pointer substrate

rather than as:
- native semantic object graph
- queryable field-aware primary representation
- storage-native execution substrate for the main document model

This means Owl's hottest stored form is still an opaque serialized Rust payload from the storage engine's point of view.

That limits how much the storage layer can do natively.

## 4. Performance Gaps

The current model leaves several kinds of performance upside unrealized.

### 4.1 Full-document materialization pressure

Because the primary stored form is a serialized document blob, reads often require:
- reading the blob
- verifying checksum
- deserializing the payload
- reconstructing a Rust `Document`

even when the query only needs a small subset of fields.

This is visible in the current read path:
- read bytes from arena: `/Users/timiadesina/Documents/owl/src/storage.rs:2105`
- deserialize full stored doc: `/Users/timiadesina/Documents/owl/src/storage.rs:2136`

That is clean, but not optimal for projection-heavy workloads.

### 4.2 Limited storage-native predicate evaluation

The storage engine does not currently understand most field semantics in the primary stored representation. As a result:
- filtering lives mostly in Rust executor logic
- field extraction lives mostly in Rust executor logic
- nested access lives mostly in Rust executor logic

This means the engine cannot yet exploit storage-native field-aware scans for the main document format.

### 4.3 Partial realization of pointer-navigation advantage

Owl's reference story wants:
- cheap traversal
- cache locality
- graph-style navigation
- fewer query-planner-style joins

Some of that is present today through `ObjRef` and `navigate_reference(...)`, but the benefit is only partial if most semantic access still requires reconstructing Rust documents from blobs.

### 4.4 Missed hot/cold layout opportunities

A native or hybrid storage representation could separate:
- hot scalar fields
- direct references
- metadata
- cold nested payload

The current blob approach makes such distinctions harder to exploit at the primary storage layer.

### 4.5 Query/runtime duplication pressure

Because semantic understanding is concentrated in Rust, any future attempt to make Zig more query-aware risks either:
- duplicating logic across Rust and Zig, or
- preserving Rust-only semantics and limiting Zig's participation

This is not a current failure, but it is a future design pressure point.

## 5. Reliability Gaps

The current architecture also raises reliability questions. Some are manageable now; others should be treated as explicit future work.

### 5.1 Blob-level integrity is stronger than semantic integrity

The current path gives us:
- payload length header
- payload checksum
- durable write ordering discipline

This is good.

But it means integrity is checked at the blob boundary, not at a richer field-aware structural boundary. If a document payload is valid as bytes but semantically stale or incompatible, recovery and interpretation rely heavily on Rust-side decoding expectations.

### 5.2 Serialization format stability risk

Because document persistence relies on Rust serialization of a Rust-side structure, long-term compatibility depends on:
- careful struct evolution
- stable serialization assumptions
- migration discipline

This is manageable, but it should be tracked explicitly as a storage contract concern.

### 5.3 Index and payload coherence is critical

The primary lookup path depends on:
- Rust-maintained index structures
- arena payloads at returned offsets
- reverse-reference metadata
- scalar/vector index metadata

The architecture relies on these moving parts staying coherent across:
- crashes
- updates
- deletes
- recovery

This is not unique to Owl, but it is especially important here because the storage engine itself is not yet the canonical semantic owner of the stored document structure.

### 5.4 Recovery semantics need continued scrutiny

The WAL-first path is promising, but recovery correctness must be judged not only by:
- byte durability

but by:
- logical document visibility
- index coherence
- reverse reference coherence
- schema metadata coherence
- id canonicalization behavior

This is particularly relevant given historical payload/index identity reconciliation logic:
- `/Users/timiadesina/Documents/owl/src/storage.rs:1998`

That code is practical, but it also signals that payload identity and canonical identity have had divergence concerns.

## 6. What The Current Model Is Best At

The current model is a strong fit when the priorities are:
- semantic evolution
- development velocity
- clear layering
- keeping low-level storage fast but simple
- avoiding premature commitment to a rigid physical document format

In other words, it is a good architecture for a system still defining itself.

## 7. What The Current Model Is Not Yet Best At

The current model is not yet the strongest form of Owl for:
- storage-native semantic execution
- selective field reads from the primary stored form
- maximizing cache-aware field layout benefits
- making references the dominant access primitive across the whole runtime
- fully exploiting Zig as the physical realization of Owl's hot semantics

This is the gap between "good architecture now" and "full Owl advantage later."

## 8. Strategic Position: Healthy But Incomplete

The best honest summary is:

> The current design is architecturally healthy, but strategically incomplete.

Healthy because:
- responsibilities are clear
- the code is evolvable
- the storage/query split is understandable
- Zig is used meaningfully without semantic overreach

Incomplete because:
- the storage engine does not yet natively realize enough of Owl's semantic model to deliver the strongest possible performance story

This is not a contradiction. It is the normal shape of a system maturing.

## 9. Likely Direction: Not Pure Blob, Not Pure Rigid Native Layout

A pure blob-only future is likely too limiting.

A pure rigid native object layout is likely too constraining for Owl's semantics, especially around:
- nested structures
- schema evolution
- graceful null behavior
- evolving query semantics

The likely best direction is hybrid.

### 9.1 Hybrid target shape

Something like:

```text
document header
+ hot scalar fields
+ direct reference slots
+ field offset metadata
+ optional projection/index hints
+ flexible nested payload tail
```

This would allow:
- selective reads for hot fields
- cheap native reference traversal
- stable storage-native metadata
- flexible representation for less-structured or evolving fields

That is a more plausible end-state for Owl than either extreme.

## 10. Staged Evolution Path

The right evolution is likely staged.

### Stage 1: Keep the current model, but document it explicitly

This document is part of that.

We should be explicit that the current model is intentionally:
- Rust semantic runtime
- Zig physical storage substrate

### Stage 2: Make the storage contract more explicit and durable

We likely need clearer, documented contracts for:
- serialized document payload format
- schema evolution expectations
- WAL and recovery invariants
- index/payload coherence guarantees
- reverse-reference integrity

### Stage 3: Introduce native or semi-native hot structure

High-value candidates:
- document metadata
- canonical identity
- typed references
- hot scalar fields
- field presence/null bitmaps

This gives real performance benefit without forcing the entire semantic model into Zig at once.

### Stage 4: Push more selective execution into storage-aware paths

Only after enough structure exists should we seriously pursue:
- storage-aware projection reads
- faster reference traversal without full materialization
- native filter acceleration for hot fields
- more efficient grouped/analytical scans over structured segments

## 11. Decision Criteria For Future Changes

We should not move toward a more native storage representation just because it feels more elegant.

We should do it when it clearly helps one or more of:
- field projection cost
- traversal latency
- analytical scan cost
- write amplification
- crash recovery clarity
- storage/index coherence guarantees
- memory overhead

If a proposed storage-native feature does not improve at least one of those in a measurable way, it is probably too early.

## 12. Immediate Gaps To Analyse More Deeply

The following deserve dedicated follow-up review.

### 12.1 Primary payload format
- How stable is the current `StoredDocument` encoding contract?
- What compatibility promises do we want to make?
- How will schema evolution interact with persisted payloads over time?

### 12.2 Recovery model
- What exactly is guaranteed after crash at each stage of write?
- What gets rebuilt versus recovered?
- Where are the remaining ambiguity points?

### 12.3 Reference realization
- Which reference operations are truly O(1) in practice?
- When do we still fall back to higher-level document lookups?
- How much full-document materialization happens during traversal?

### 12.4 Index/storage coherence
- How are document index, reverse refs, scalar indexes, vector metadata, and arena payload kept consistent?
- Which invariants are already enforced?
- Which are convention rather than guarantee?

### 12.5 Hybrid layout candidates
- Which fields should be native first?
- Which remain blob-tail fields?
- What should qualify a field as "hot"?

## 13. Current Recommendation

The recommendation at this stage is:

1. Keep the current architecture.
2. Do not treat the current blob-on-zig design as a mistake.
3. Treat it as a deliberate and useful intermediate stage.
4. Make the storage contract and recovery guarantees more explicit.
5. Plan a hybrid next stage instead of staying blob-only forever.

## 14. Bottom Line

The current Owl architecture is good enough to be respected.

It is:
- coherent
- practical
- evolvable
- focused

But it is not yet the strongest possible technical realization of Owl's performance thesis.

Today, Zig is the storage substrate.
Later, for Owl to fully cash in on its promise, Zig will likely need to become the physical realization of at least Owl's hottest semantic structures, while Rust continues to own the language and much of the runtime semantics.

That is the gap.
That is also the opportunity.
