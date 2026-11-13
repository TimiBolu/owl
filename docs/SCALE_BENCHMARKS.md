# Owl single-node scale benchmarks

This document records reproducible scale results rather than isolated latency
claims. The `owl-scale` release binary generates one bounded dataset at a time,
reports logical and physical bytes, checkpoints and reopens Owl, measures the
first and warmed batch-native broad projection, and removes the complete
temporary directory before starting the next case.

## Running the ladder

```sh
cargo run --release --bin owl-scale -- \
  --rows 10000,100000,1000000 \
  --record-bytes 64,1024 \
  --samples 5 \
  --max-case-gib 2 \
  --output /tmp/owl-scale.json
```

`--max-case-gib` is a physical high-water budget. The harness conservatively
estimates record overhead before starting a case, samples the complete
temporary directory after ingest batches and storage transitions, and aborts
if observed bytes exceed the budget. Reports enumerate every segment,
manifest, record delta, index delta, WAL, and superblock file. Temporary
datasets use an operating-system temporary directory and cleanup is verified
after each case.

The graph-specific ladder uses a bounded fan-out dataset, persists its reverse
adjacency, reopens the same immutable checkpoint repeatedly, measures cold and
warm target lookups, applies a bounded mutation overlay, and verifies cleanup:

```sh
cargo run --release --bin owl-reference-scale -- \
  --sources 100000,1000000 \
  --targets 1000 \
  --mixed-reference-stride 4 \
  --mutations 100 \
  --samples 7 \
  --reopen-samples 7 \
  --max-case-gib 2 \
  --output /tmp/owl-reference-scale.json
```

Every source has a `parent` edge. By default every fourth source also has a
`related` edge, exercising 1.25 edges per source across two independently
queryable paths. `--mixed-reference-stride 0` selects the single-path control.
The report records source, target, edge and path counts, physical and peak
bytes, first/min/median/max reopen latency, cold and warm lookup latency, target
sweep time, mutation time, post-mutation cardinality, and a second full
checkpoint over the persisted base plus that mutation overlay. Reopen is
sampled against one checkpoint so run variance is visible instead of being
mistaken for a format effect.

## First ladder: row-directory engine

Run date: 2026-07-26. Release build, local Apple Silicon development machine.
The query was:

```oql
events.where { .sequence >= 0 }.select { message }
```

The selected message remained small while the unselected padding produced the
requested record width.

| Rows | Value bytes/row | Logical data | Checkpoint files | Physical/logical | Ingest | Reopen | First scan | Warm scan | Warm throughput |
|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| 10K | 64 B | 0.94 MiB | 2.42 MiB | 2.65× | 91 ms | 18 ms | 0.87 ms | 0.53 ms | 18.92M rows/s |
| 100K | 64 B | 9.16 MiB | 41.09 MiB | 4.49× | 1.20 s | 199 ms | 13.05 ms | 11.48 ms | 8.71M rows/s |
| 1M | 64 B | 91.55 MiB | 412.99 MiB | 4.51× | 52.82 s | 2.47 s | 195.23 ms | 173.17 ms | 5.77M rows/s |
| 10K | 1 KiB | 10.07 MiB | 11.58 MiB | 1.15× | 128 ms | 25 ms | 1.60 ms | 0.58 ms | 17.28M rows/s |
| 100K | 1 KiB | 100.71 MiB | 188.93 MiB | 1.88× | 1.50 s | 270 ms | 18.39 ms | 17.76 ms | 5.63M rows/s |
| 1M | 1 KiB | 1007.08 MiB | 1.58 GiB | 1.60× | 241.98 s | 9.97 s | 450.52 ms | 232.66 ms | 4.30M rows/s |

All six dataset directories reported `cleanup_verified=true`. The 1M × 1 KiB
case was observed externally peaking near 2.0 GiB while automatic maintenance
transitioned between files; this observation caused the harness budget to be
changed from a logical estimate to a conservative physical high-water limit.

## Conclusions

The present row-directory path scales worse than linearly:

- For 64-byte values, 100× more rows caused approximately 328× more warmed
  scan time.
- For 1 KiB values, 100× more rows caused approximately 402× more warmed scan
  time.
- Warm throughput fell from 17–19M rows/s at 10K to 4.3–5.8M rows/s at 1M.
- Record width affected a projection that did not request the padding field,
  proving that unrelated row data still expands the cache and recovery working
  set.
- Reopen reached 9.97 seconds at 1M × 1 KiB because recovery hydrates generic
  documents into the resident object directory.
- Alternating crash-safe checkpoint generations and record deltas can
  temporarily coexist. Physical budgeting must therefore track the whole
  directory, not only the active segment or logical payload.

These results reject extrapolation from the former 10K benchmark. The next
scan engine must operate over immutable typed column blocks with selection
vectors, segment statistics, bounded residency, and parallel morsels. The
generic `HashMap<String, Value>` document directory remains useful for the
mutation and compatibility boundary, but it cannot remain the production
broad-scan representation.

## First typed-segment implementation slice

The implemented nucleus is deliberately narrower than a complete analytical
engine:

1. Full checkpoint output groups the current schema version into ID-ordered
   immutable segments capped at 65,536 rows.
2. Every segment stores identity and system-metadata arrays, presence and null
   bitmaps, fixed-width typed arrays, and offset-plus-byte string blocks.
   Timestamps retain nanosecond precision.
3. Integer min/max statistics can reject or accept an entire segment before
   row evaluation.
4. Fully covered checkpoint scans expose the immutable segment plus selected
   row indices through `DataChunk`; projected strings remain borrowed through
   Loggr JSON serialization.
5. Writes after the typed checkpoint enter a small ID-ordered row overlay.
   Inserts and updates carry live `ObjRef`s; deletes carry tombstones. Scans
   merge that overlay directly with typed segment order, evaluating only
   changed rows through the document representation.
6. OWLC v2 persists an explicit authority bit. A row group is authoritative
   only when every compiled field has an exact typed codec. Its rows are then
   omitted from OWLR; nested/list/vector/global-reference or older-version
   rows remain in the row overflow region.
7. LZ4 is selected independently for each sufficiently large column values
   block and string byte block, and only when the encoded frame is smaller.
   Checksums cover the stored bytes and decode bounds are authenticated before
   allocation.
8. Recovery reconstructs authoritative rows from typed arrays, but still
   hydrates every `Document`. Lazy recovery is the next memory-bound step.
9. Typed bytes are included in the crash-safe segment checksum before an
   alternating manifest references them.

The v3 checkpoint envelope uses a 72-byte header and a 64-bit typed-region
length. Readers retain support for the 64-byte v2 envelope and OWLC v1
accelerators. Authority is explicit rather than inferred from missing row
bytes, so corruption cannot silently turn an overflow row into a typed row.

### First measured typed result

Run date: 2026-07-26, same machine and query as the first ladder, 100K ×
64-byte case, seven retained warm samples:

| Rows | Typed groups | Typed coverage | Batch assembly | Borrowed value scan | Value throughput | Physical files | Physical/logical |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 100K | 2 | 100% | 0.476 ms | 5.224 ms | 19.14M rows/s | 64.19 MiB | 7.01× |

“Batch assembly” creates selection vectors and projected segment views but does
not touch selected string bytes. “Borrowed value scan” additionally walks all
100K projected rows and hashes all 1.89 MB of selected message bytes without
allocating public documents. The distinction prevents a lazy-result number
from being reported as physical value throughput.

With 100 post-base updates (0.1% of the collection), the same bounded case
measured 0.946 ms batch assembly and 5.720 ms borrowed value scan, or 17.48M
rows/s. An initial merge prototype walked the entire live row B-tree and
measured 11.18 ms. The retained implementation instead merges the typed ID
stream with the small ordered overlay, so unchanged rows do not perform
document-directory lookups or hash-set probes. Insert/update/delete ordering,
reopen, and full-checkpoint refresh are covered together by a regression test.

An intermediate run exposed a real bug: only 30,368 of 100K rows appeared in
typed segments because clean base record bytes had been evicted before
replacement-checkpoint construction. That run safely fell back to rows and
measured 15.24 ms. Checkpoint construction now derives schema versions from the
complete durable record set being published; a regression test covers
replacement checkpoints after base eviction.

The 7.01× physical/logical ratio was not acceptable as a final layout. That
migration slice stored authoritative row records and typed columns together,
while two crash-safe checkpoint generations could coexist. It proved the scan
architecture and motivated the authority/compression step measured next.

### Authoritative typed storage result

Run date: 2026-07-27, same 100K × 64-byte workload, five retained warm
samples:

| Rows | Authoritative / overflow | Batch assembly | Borrowed value scan | Value throughput | Retained physical | Physical/logical | Reopen |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 100K | 100K / 0 | 0.470 ms | 5.187 ms | 19.28M rows/s | 14.09 MiB | 1.54× | 175.71 ms |

The retained footprint includes both alternating crash-safe checkpoint
generations. It fell from 64.19 MiB to 14.09 MiB (78% lower) because the
100K scalar rows are no longer also stored as OWLR records. Query timings are
effectively unchanged, so the space reduction did not move work into the hot
scan. Peak observed bytes were 26.58 MiB during checkpoint rotation.

### Lazy typed-row recovery

Authoritative rows now enter live state as `(segment, row)` locations.
Recovery builds external-ID and ordered-scan directories from segment metadata
without constructing field maps. Point reads materialize and cache only the
requested `Document`; typed scans do not populate that cache. Reverse-reference
recovery walks only typed reference columns, and structural index recovery uses
identity directories rather than forcing row materialization.

On the same 100K case, immediately after reopen:

- authoritative typed rows: 100,000;
- resident general Documents: 0;
- lazy typed rows: 100,000;
- median warm batch assembly: 0.474 ms;
- median selected-value traversal: 5.289 ms;
- reopen: 173.75 ms.

The residency goal is achieved, but reopen latency is essentially unchanged
from 175.71 ms. Typed decoding plus eager construction of the external-ID,
ordered-ID, and `ObjRef` location maps now dominates. Claiming a startup-speed
win would be dishonest. The next step for startup and identity-memory scaling
is a persisted, paged identity directory that can be mapped or loaded by page;
the current in-memory location map is the correct lazy-object foundation, not
the final billion-row directory.

### Open versus audit boundary

Ordinary reopen no longer streams the complete checkpoint merely to recompute
its whole-file checksum. Explicit integrity checks and reclamation still
perform that audit; ordinary reads validate record, index-page, and typed
row-group checksums as those structures are decoded. OWLS v4 independently
checksums the bounded header/catalog/directory root needed to interpret the
file, so fast open does not trade away fail-closed root validation. OWLC v3 row
groups now also have a descriptor-only discovery path that reads their fixed
64-byte headers and skips payloads. Their compact identity prefixes carry
independent checksums and retain UTF-8 IDs in one byte array, making them safe
inputs for the forthcoming persisted lookup directory without per-row
`String` allocation.

This was deliberately measured before further format work. Fresh 100K ×
64-byte cases reopened in 154.85–179.02 ms. The final OWLS v4 / OWLC v3 run
reported 179.02 ms, 14.09 MiB / 1.54× physical size, 0.552 ms median batch
assembly, and 5.330 ms selected-value traversal. The reopen range is not a
meaningful improvement over 173.75 ms. It demonstrates that whole-file audit
was not the dominant cost; eager typed decode and the three per-row identity
directories are.

The micro-reopen acceptance condition therefore remains: install persisted
row-group and identity/locator roots without decoding column payloads or
allocating one `String`, hash entry, and B-tree entry per base row. Only WAL
delta identities may be rebuilt eagerly.

### File-backed row-group result

Recovered OWLC row groups are now file-backed handles. Immediately after the
100K reopen, `typed_loaded_segments` was 0 of 2: no column body had been read or
decompressed. The current identity prefix was 7,000,008 bytes and recovery
still converted its rows into the legacy external-ID hash, ordered-ID B-tree,
and `ObjRef` location hash.

That transitional run reopened in 189.38 ms. Its first projected scan took
19.00 ms because it performed the deferred read, checksum, and decompression;
median warm batch assembly was 0.481 ms and selected-value traversal was
5.205 ms. The result is a successful lazy-I/O boundary but a failed
micro-reopen target. The next format step is a small persisted fence root plus
paged ID/locator leaves, after which base rows must not be inserted into the
three in-memory directories.

### Base-directory removal

The external-ID hash and collection-order B-tree no longer contain
authoritative base rows. Segment first/last fences route point lookups to a
binary search over persisted ID bytes; immutable segment order is the scan
base, merged with only overflow rows and WAL upserts/tombstones.

The bounded 100K progression was:

| Layout | Reopen | ID hash rows | Ordered-tree rows | ObjRef locator rows |
|---|---:|---:|---:|---:|
| Lazy columns, legacy directories | 189.38 ms | 100,000 | 100,000 | 100,000 |
| Segment fences, no ID hash | 182.73 ms | 0 | 100,000 | 100,000 |
| Segment order, no base B-tree | 152.12 ms | 0 | 0 | 100,000 |
| Dense locator, no locator hash | 147.11 ms | 0 | 0 | 100,000 |
| Split lookup leaf (OWLC v4) | 88.11 ms | 0 | 0 | 100,000 |
| Packed 64-bit locator slots | 85.05 ms | 0 | 0 | 100,000 |
| Persisted locator leaf (OWLS v5, controlled) | 25.26 ms | 0 | 0 | 100,000 |
| File-backed ID leaves + fence root | 0.97 ms | 0 | 0 | 100,000 |

OWLC v4 separates object references, external-ID offsets, and external-ID bytes
from document versions and timestamps. The independently checksummed lookup
leaf fell from 7,000,008 to 3,800,008 bytes for this case; the cold row metadata
stays with the lazily loaded column body. That run's in-memory graph locator
used one packed `u64` per allocated object rather than
`Option<(usize, usize)>`. Local graph navigation remained O(1), no typed segment
was decoded at reopen, and warm batch assembly was 0.295 ms. Fresh OWLL v3
leaves now use checked four-byte slots while retaining reads for the earlier
eight-byte formats.

OWLS v5 now persists the packed locator as an independently checksummed OWLL
leaf. Ordinary recovery installs the immutable leaf directly and retains only
post-checkpoint overrides in a hash overlay; older OWLS versions retain the
compatible rebuild path. The scale harness now disables automatic checkpoints
so each controlled case measures one explicit base publication rather than an
unknown mixture of maintenance generations.

The controlled results prevent overclaiming:

| Rows | Resident ID-leaf bytes at open | Reopen | First scan | Warm batch | Physical/logical |
|---:|---:|---:|---:|---:|---:|
| 100,000 | 0 | 0.97 ms | 18.68 ms | 0.469 ms | 0.99× |
| 1,000,000 | 0 | 11.93 ms | 207.80 ms | 5.010 ms | 1.00× |

OWLL v2 carries authenticated first/last ID fences for each authoritative
segment. Ordinary open installs those roots and leaves every ID/offset leaf
file-backed; point lookup loads only the routed segment leaf, while an ordered
scan loads leaves as it advances. Audit still reads and validates every leaf.

An unrelated empty-work dependency was also removed: reverse-reference
recovery no longer builds a complete object identity map when there are zero
reference targets. That path had defeated lazy IDs for non-reference schemas.

The first scan now owns the deferred leaf I/O and checksum work. Warm batch
assembly scales about 10.7× for 10× rows. Reopen scales about 12.3×, narrowly
outside the 12× acceptance gate; this is recorded as near-gate, not a pass.

Reverse adjacency no longer adds a global recovery pass. Full checkpoints
publish a hidden paged index keyed by target `ObjRef` plus field path, with
ordered source `ObjRef` postings. Recovery installs page descriptors only;
reverse lookup reads pages for one target prefix, and WAL puts/deletes update
only changed edge keys in an overlay. Checkpoints predating this root use one
legacy derivation and upgrade on their next full checkpoint.

Each current-format page starts with a compact authenticated routing prefix:
index identity, first/last key fences and posting count. Recovery validates and
retains this prefix while seeking over the entry payload. The independent full
page checksum is deferred until a query reads that page. Legacy pages remain
readable through the former eager decoder. This distinction matters: before
the routing prefix, the nominally file-backed reverse index still decoded and
cached every page during open.

### Mixed-reference controlled ladder

Run date: 2026-07-27. Release build on the same local Apple Silicon development
machine. Each source had a `parent` edge and every fourth source had a
`related` edge; 100 sources were then rewritten to another parent.

| Sources | Edges | Initial checkpoint | Base + 100 mutations checkpoint | Reopen median (range) | Cold parent lookup | Warm parent lookup | 100 rewrites |
|---:|---:|---:|---:|---:|---:|---:|---:|
| 100K | 125K | 0.722 s | 0.493 s | 0.602 ms (0.571–0.887) | 1.337 ms / 100 rows | 0.020 ms | 15.84 ms |
| 1M | 1.25M | 12.313 s | 4.679 s | 6.340 ms (5.804–6.945) | 15.034 ms / 1,000 rows | 0.420 ms | 24.70 ms |

Both cases opened with zero decoded typed segments, zero resident ID-leaf
bytes and zero resident documents, and both verified complete temporary-data
cleanup. Initial physical sizes were 9.98 MiB and 98.73 MiB; observed
transition high-water sizes were 25.24 MiB and 250.12 MiB. The second full
checkpoint deliberately exercises persisted typed base plus a small encoded
mutation overlay, not another initial-WAL special case.

Profiling separates two cold-lookup costs. In a traced 100K run, routing and
the first reverse page consumed 1.144 ms while source-ID shaping consumed
1.142 ms. In a traced 1M run, those phases were 1.443 ms and 38.485 ms
respectively. The reverse range itself is therefore bounded; a uniformly
scattered target forces the public edge API to touch lazy identity leaves
across the source collection to return external IDs. Warm lookup avoids that
I/O. An internal ObjRef-native traversal path is the appropriate follow-up for
OQL execution; duplicating every external ID into every reverse edge would
trade this cold read for material storage and write amplification.

Before bounded-decoding compaction, two million-row full-checkpoint samples
took 167.63 and 156.86 seconds. The latter spent approximately 121 seconds
synchronously destroying a temporary recovery `State` containing one million
decoded documents and their field maps.

The replacement reopens the prior checkpoint lazily, coalesces sealed-WAL
puts/deletes as authenticated encoded records, merges base identities with
that overlay in external-ID order, and decodes at most one 65,536-row typed
group while rebuilding typed segments plus scalar and reverse indexes. The
controlled initial checkpoint fell from 156.86 to 12.31 seconds (12.7×), and a
subsequent full checkpoint over the million-row typed base plus 100 mutations
took 4.68 seconds. No destructor work was detached or hidden.

The strict memory-bound follow-up is now implemented. WAL changes coalesce in
a 16-MiB ID-ordered buffer and spill as checksummed sorted runs. A k-way cursor
merges those runs with an iterator-native base identity cursor; no complete
base ID sequence is materialized. Base suppression is a collection-partitioned
bitset (one bit per allocated object slot), not a hash entry per changed row.
An atomically published run manifest makes temporary files discoverable after
a process interruption; normal completion/error unwinding and the next open
remove them.

This bound is not free. Controlled release results after the change were:

| Sources | Peak WAL-change buffer | Compressed temporary runs | Initial checkpoint | Base + 100 mutation checkpoint |
|---:|---:|---:|---:|---:|
| 100K | 16.00 MiB | 3 / 6.30 MiB | 0.873 s | 0.528 s |
| 1M | 16.00 MiB | 22 / 59.47 MiB | 12.806 s | 4.671 s |

Runs use Owl's native versioned frame, 256-KiB independently checksummed LZ4
blocks, and compact native change entries rather than per-entry generic
serialization. The merge uses a minimum heap, making head selection
`O(rows × log(runs))` rather than comparing every row with every run.
More than 32 runs are consolidated in bounded-fan-in passes, so checkpoint
memory and open file descriptors do not grow with the total run count.

The earlier in-memory encoded overlay measured 0.722 s and 12.313 s for the
initial checkpoints. The strict-bound implementation is therefore 0.151 s
slower at 100K and 0.493 s slower at 1M, rather than the 4.41-second regression
of the first uncompressed spill implementation. The persisted-base checkpoint
is effectively unchanged because its 100 mutations form one roughly 6-KiB
run. Reopen measured 0.627 ms at 100K and 6.507 ms at 1M. Run-byte telemetry
is emitted by the checkpoint trace; the benchmark's sampled directory
high-water does not claim to include this synchronous temporary-run peak.

A special initial-build checkpoint path was considered but is not implemented:
the same general compactor is now within roughly 4% of the former unbounded
1M result, uses less scratch than the final 98.73-MiB database, and preserves
one correctness path for initial and established databases.

### 10M bounded-ingestion result

Run date: 2026-07-27, 8-GB Apple laptop, release build. The one-shot harness
was stopped after reaching 1.21 GB RSS early in ingestion because its resident
document set projected beyond physical memory. The retained run used
`--ingest-checkpoint-rows 1000000`: every million sources received a full
checkpoint followed by a lazy reopen. This bounds resident ingestion memory
but deliberately performs cumulative work over 1M + 2M + … + 10M rows.

```text
sources:                         10,000,000
edges:                           12,500,000
record payload:                  64 bytes
ingestion checkpoints:           10
pure ingestion time:             189.689 s
cumulative ingestion checkpoint: 789.502 s
total wall time:                 1,295.62 s
maximum RSS:                     2,285,633,536 bytes
peak process footprint:          6,948,797,632 bytes
swaps:                           0
reopen median:                   48.844 ms
cold reverse lookup:             310.402 ms / 10,000 rows
warm reverse lookup median:      4.368 ms / 10,000 rows
100 rewrites:                    83.679 ms
base + 100 mutation checkpoint:  96.130 s
post-mutation physical bytes:    2,067,216,914
cleanup:                         verified
```

Against the controlled 1M case, reopen scaled 7.51× for 10× rows and the
32-target sweep scaled 11.75×. Pure ingest scaled 16.36×, warm reverse lookup
23.0× while returning 10× as many postings, and the post-mutation full
checkpoint scaled 20.58×. Storage after two retained checkpoint generations
scaled essentially 10× (207.0 MB to 2.067 GB). The run therefore proves the
capacity and lazy-open path on this machine, but it does **not** pass a general
linear-scaling claim. Full-checkpoint validation/installation and scattered
external-ID shaping for high-degree reverse results are the next measured
targets.

The remaining acceptance gates are:

- 10× more rows produces no more than 12× warm-scan time from 100K through 10M;
- a one-column projection over 64-byte and 1-KiB records differs by no more
  than 15% once the selected bytes are equal;
- scan memory is bounded by segment and output batch sizes rather than total
  row count;
- [achieved] reopen does not hydrate the full object collection;
- output is identical for missing versus explicit null, system fields,
  updates, deletes, checkpoint reopen, and mixed schema versions;
- every scale case reports and enforces physical high-water bytes and verifies
  cleanup.

### Checkpoint installation correction

A 2026-07-28 controlled 1M-source rerun separated publication, installation,
and image construction:

```text
segment size:                         98.73 MiB
checkpoint image construction:        5.02-6.08 s
segment publication:                  0.895-1.126 s
initial resident-directory exchange:  0.002 ms structural / 5.07 s destruction
100-mutation resident retirement:     0.32-0.60 ms
100-mutation live install total:       5.6-7.9 ms
100-mutation checkpoint end-to-end:    4.23 s
```

Publication no longer rereads the complete segment for a second checksum, and
live installation no longer decodes, row-validates, or walks the immutable
typed base. The post-mutation install is consequently proportional to segment
descriptors plus changed resident rows.

This does **not** make full checkpoints scale-ready. The 4.23-second result
still rebuilds the complete 1M-row image, and initial ingestion still creates
and later destroys one heavyweight resident `Document` per row. The next scale
boundary is partitioned immutable generations, authenticated paged locators,
and typed mutable row groups, as specified in
`STORAGE_ENGINE_ARCHITECTURE.md`.

### File-backed paged locator

OWLL v4 separates its authenticated root/fences from independently checksummed
4,096-slot pages. Fresh publication and reopen retain no page payload until an
`ObjRef` lookup touches it; a fixed 64-page cache bounds payload residency at
approximately 1 MiB. Both scale harnesses report cached pages and bytes.

A bounded 100K-source release check measured:

```text
reopen:                         0.863 ms
locator pages/bytes at open:    0 / 0
typed segments loaded at open: 0
resident typed identity bytes: 0
cold reverse lookup:           2.063 ms
warm reverse lookup:           0.041 ms
physical database:             9.59 MiB
cleanup:                       verified
```

This validates the open-memory boundary. Locator construction now uses a
bounded external sort and authenticated page spool: at most 1,048,576 fixed
ten-byte tuples are sorted in memory, merge fan-in is capped at 32, and only
one 4,096-slot page is shaped at a time. A small checksummed journal makes all
deterministically named run/page artifacts reclaimable after process death.
This is a locator-specific bound; the current checkpoint image still owns its
typed segment payloads in memory, so it is not yet a full billion-row
checkpoint-memory claim.

After adding the durable interrupted-build journal, the same bounded 100K
release case measured:

```text
ingest:                         1.077 s
initial full checkpoint:        0.916 s
reopen median:                  0.500 ms
locator pages/bytes at open:    0 / 0
locator pages/bytes after cold: 25 / 404,004
cold reverse lookup:            1.659 ms
warm reverse lookup median:     0.022 ms
100 mutations:                  13.225 ms
post-mutation full checkpoint:  535.114 ms
physical database:              9.59 MiB
peak observed storage:          25.24 MiB
cleanup:                        verified
```

The journal and bounded builder introduce no visible regression. The
post-mutation checkpoint still rewrites the complete base for only 100 changed
rows; immutable manifest-owned generations are the next architectural target.

### Change-proportional manual checkpoint

Manual `checkpoint()` previously bypassed Owl's incremental maintenance path
and forced a full reconstruction. It now uses the same byte-based decision as
automatic maintenance; `compact()` is the distinct forced-compaction API.
Repeating the identical 100K case produced:

```text
ingest:                         1.088 s
initial full checkpoint:        1.034 s
reopen median:                  0.496 ms
cold reverse lookup:            1.747 ms
warm reverse lookup median:     0.023 ms
100 mutations:                  17.390 ms
post-mutation checkpoint:       56.119 ms
physical before mutations:      10,060,638 bytes
physical after checkpoint:      10,114,534 bytes
cleanup:                        verified
```

For 100 changed rows, checkpoint latency fell from 535.114 ms to 56.119 ms
(9.5×) and durable growth fell from a second complete 9.59-MiB base to 53,896
bytes. Query and reopen timings stayed within normal run variance. The
remaining 56 ms is not the target state: cumulative WAL/record/index roots are
still rewritten and recovery-shaped on every incremental publication. The
next generation format must make flush cost proportional to the new generation
without copying prior overlay bytes.

### Independent immutable flush generations

That cumulative-copy boundary is now removed for fresh checkpoints. Each
post-base checkpoint publishes only its newly sealed WAL file, a paged local
record root for that generation's changed compiled records, a paged `OWLI`
root for scalar and reverse-reference index changes, and a small `OWGR` root
listing immutable generation descriptors. The root authenticates the base
identity, ordered sequence and LSN ranges, the complete WAL, and the record
root's length and page-fence directory. Record payload pages are independently
checksummed and verified when read; explicit integrity audit still scans every
page, but normal reopen does not. The existing double-buffered manifest remains
the final authority switch. Format 3 retains one fence per roughly 16 KiB page,
not one directory entry per changed object. Reads resolve generation record
roots newest-first before the base, so clean compiled bytes are evicted without
allowing stale base values to win. Recovery installs local OWLI patches instead
of rebuilding those indexes from every changed document. Scale reports expose
generation count plus WAL, record-root, and index-root bytes separately.

The list is capped at 32 generations and by total generation bytes. Explicit
or threshold compaction folds the list into a new OWLS base and reclaims every
old generation after the new manifest is durable. Tests cover ordered replay,
scalar-index/catalog mutations, updates and deletes, missing/corrupt generation
files, corrupt record and index roots, corrupt and semantically non-contiguous
descriptor roots, no-op checkpoints, reverse-reference recovery, compaction
reclamation, and cleanup of all companion files from an interrupted unpublished
generation.

No performance result is claimed for this format yet. It changes the measured
operation enough that the 100K, 1M and later scale rows must be rerun rather
than compared indirectly with the former cumulative-root numbers. Its known
temporary cost is recovery replay and reconstruction of general `Document`
maps for uncompacted changes. Compiled record bytes and index patches are now
file-backed; typed local row groups and identity locators are the next
implementation boundary.
