# Owl Storage Engine Architecture

Status: accepted direction; implementation has started  
Scope: reliable single-node engine, implemented entirely in Rust

## 1. Decision

Owl has one storage engine. It is not a versioned alternative to the previous
engine and it has no Zig or other native storage dependency. Rust owns the whole
correctness boundary:

- object identity and the object directory
- record encoding and allocation
- write-ahead logging and commit ordering
- primary, secondary, unique, and reference indexes
- checkpoints, recovery, compaction, and retention
- filesystem error handling and durability tests

Loggr is the first forcing workload, not the physical definition of Owl. The
engine must also serve ordinary transactional object workloads such as finance,
games, and e-commerce.

## 2. Product boundary

The first trustworthy release supports:

- one database process on one node
- one serialized writer with group commit
- concurrent snapshot readers
- atomic batches spanning objects, constraints, and indexes
- stable local object references
- mutable and append-oriented collections
- schema-compiled records
- primary, unique, non-unique, composite, ordered, and reference indexes
- checksummed WAL frames, recovery, checkpoints, and segment reclamation
- explicit corruption and I/O errors

The first release does not include distributed consensus, cross-node
transactions, general multi-writer MVCC, vector indexes, automatic graph
partitioning, or a general analytical column store.

## 3. Workload model

| Profile | Representative operations | Storage property exercised |
|---|---|---|
| Loggr | batch append, latest-N, time range, fingerprint, retention | append policy, time pruning, compression |
| Ledger | atomic transfer and ledger append | durability, decimal, unique constraints, atomic batches |
| Game | entity lookup, component update, inventory traversal, TTL | point access, partial decode, references, latency |
| Commerce | order creation, inventory update, customer/order traversal | mixed reads/writes, composite indexes, graph paths |

Owl does not need to beat a mature key/value engine for every opaque KV
microbenchmark. It must demonstrate an advantage where schema, typed values,
projections, relationships, and query planning supply information a KV engine
does not possess.

## 4. Logical storage model

### 4.1 Collection

A schema owns a collection. Every collection has a stable 16-bit internal ID and
one declared physical policy:

- `Mutable`: general records with updates and deletes.
- `Append`: immutable or rarely changed events, efficient batching and retention.

Policies do not alter OQL semantics or object identity. More specialized
policies require measured evidence and a format review.

### 4.2 Object identity

An `ObjRef` is a packed identity, never a byte offset:

```text
+-----------------+----------------------------------+------------------+
| collection (16) |          object id (32)          | generation (16)  |
+-----------------+----------------------------------+------------------+
```

Zero is null. The object directory maps an `ObjRef` to its current physical
record location. Compaction can move records without rewriting references.
Generation rejects stale identities when an object ID is eventually reused.
Updating an object does not change its `ObjRef`.

### 4.3 Object directory

The directory is a dense, mmap-friendly array per collection. An entry contains:

```text
generation | segment_id | record_offset | record_length | flags | commit_lsn
```

Readers resolve a reference with a collection-directory lookup followed by a
record access. A directory page is published only after its transaction commits.

## 5. On-disk layout

The target database directory is:

```text
owl.db/
  MANIFEST.0
  MANIFEST.1
  wal/
    0000000000000001.wal
  schemas/
  collections/
    <collection-id>/
      directory/
      segments/
  indexes/
    <index-id>/
  tmp/
```

The two manifests use sequence numbers and checksums. A new manifest is written,
synced, and atomically selected before files reachable only from the old
manifest may be reclaimed.

The first implemented checkpoint layout is deliberately smaller than this target
directory. For a database path `<db>`, it uses alternating
`<db>.manifest.0/1`, `<db>.segment.0/1`, `<db>.record-delta.0/1`, and
`<db>.index-delta.0/1` sidecars plus the active/sealed WAL files. Each
segment is a complete point-in-time snapshot containing cold catalog metadata,
a fixed-width object directory, and contiguous raw `OWLR` records. This bounds
WAL replay now. The matching `OWLD` artifact is a versioned, checksummed COW
record root. Format 3 keeps only ordered page fences in its root directory;
each independently checksummed record page contains its own fixed-width object
directory and record payload (16 KiB targets, with overflow pages for larger
records). A lookup binary-searches the resident fences by stable `ObjRef`, reads
one page, then binary-searches that page's entries. Resident locator metadata is
therefore proportional to page count rather than object count. Format 1 flat
roots and format 2 roots with a global object directory remain readable. A full
checkpoint publishes an
empty overlay rooted at the full segment's LSN. Each incremental checkpoint
merges changed records and tombstones into the prior overlay, so the result
always describes the complete difference between the unchanged base segment and
the manifest's target LSN.

`RustStore` retains a file-backed handle for the record root selected by the
durable manifest. A format 3 handle keeps only the authenticated header and page
fences in memory and opens, seeks, and reads one page for a clean incremental-record point
projection. Objects changed after the published checkpoint are marked dirty and
must use the live COW record instead, so a stale page cannot shadow a concurrent
or newer commit. A matching `OWLS` handle keeps the base object directory in
memory and seeks directly to the base record when the overlay has no entry.
Both handles are built before, and swapped together after, manifest durability;
readers therefore cannot combine base and overlay files from different
checkpoint generations. The base directory is trusted only after whole-segment
manifest verification; each fetched record also has its own checksum and its
embedded `ObjRef` is checked before decoding. After open and after each durable
checkpoint, clean compiled record bytes are evicted from the live COW directory;
only dirty/new records and records frozen for an in-progress publication remain
materialized. Lazy-only deletes are tracked as tombstones even though no compiled
bytes are resident. The decoded document directory remains materialized for the
current scan and mutation paths. Off-lock full-checkpoint reconstruction also
evicts clean compiled bytes from its temporary `State`; snapshot assembly reads
unchanged records from the durable base/overlay handles and takes changed records
from sealed-WAL materialization. The streaming file codec removes the
whole-segment input/output blob, and its one-shot record reader moves each
verified record directly into the reconstructed state. Only the record currently
being read is transiently materialized outside that state.

The matching `OWLI` artifact is the scalar ordered-index root. It contains
explicit key patches whose values are complete ordered posting lists, plus key
tombstones and index reset/delete operations. Reset and delete operations make
index creation, definition replacement, deletion, and recreation unambiguous.
Incremental publication coalesces repeated changes to the same index key across
checkpoints; a full checkpoint resets `OWLI` to an empty overlay over the index
blocks in its base segment.

Manifest format 2 separates the manifest-copy slot from the referenced base
segment slot, marks whether native record and index roots are present, and
optionally records the exact length/checksum of a cumulative WAL delta. Recovery remains
backward-compatible with format 1. For a v2 delta root it verifies the older base
segment, verifies and replays the referenced WAL delta, reconstructs all records
from the base plus `OWLD`, and requires both recovery paths to produce the same
record set at the manifest checkpoint LSN and live-object count. It also applies
`OWLI` to the base index blocks and requires the resulting complete key/posting
maps to equal the WAL-derived indexes. This makes a missing, corrupt, stale, or
incomplete record or index root fail closed. WAL replay still reconstructs the
catalog in this stage; using native roots as the primary recovery path is the
next optimization.

Automatic checkpoint publication emits a full root only when no base exists or
compaction is due. Otherwise it appends the newly sealed WAL to the previous
cumulative delta, writes it into the inactive fixed slot, merges and writes the
matching cumulative `OWLD` root, and publishes a v2 manifest that continues to
point at the unchanged full base segment. Scalar-index key changes are captured
from per-index dirty sets and merged into the cumulative `OWLI` root in the same
publication. The obsolete cumulative WAL-delta slot is removed only after the
new manifest is durable. Alternating native-root slots bound their space while
preserving the roots referenced by the current manifest.

`RustStore::reclaim_obsolete_files` provides an explicit reachability boundary
for the fixed-slot layout. It selects the newest valid manifest, removes and
syncs obsolete manifest authority first, and only then removes segment, record
root, index root, and cumulative-WAL artifacts not reachable from the retained
manifest. Active and sealed WAL generations are outside this operation. This
ordering is conservative under interruption: a surviving manifest never names
a file reclaimed by the operation.

`RustStore::check_integrity` is a read-only offline verifier, while
`verify_integrity` additionally requires the durable reconstruction to equal an
open store's live state under the commit/checkpoint boundary. The verifier walks
the authoritative manifest, full segment, `OWLD`, `OWLI`, cumulative WAL, sealed
WAL, and active WAL; rejects torn authenticated tails; validates document and
`ObjRef` identity; audits scalar-index keys, postings, uniqueness, and complete
coverage; and recomputes reverse-reference edges from live documents.

The same operations are exposed for operators:

```text
owl-admin storage check <db-path>
owl-admin storage reclaim <db-path> --size-mb 128
```

The cumulative-root path remains readable for format compatibility, but fresh
checkpoints no longer extend it. After the first OWLS base, maintenance writes
the newly sealed WAL as one independently checksummed immutable generation.
It then writes an `OWGR` root containing the base identity and the ordered
generation descriptors, followed by the existing inactive authority manifest.
That publication order makes the old set or the complete new set visible,
never a partial set. `OWGR` requires contiguous LSN ranges, increasing
sequences, and matching terminal sequence/LSN values.

The generation list is capped at 32, and its total encoded WAL bytes are
compared with the larger of 64 KiB or half the base size. Crossing either bound
selects bounded full compaction. Manual and automatic `checkpoint()` share the
policy; `compact()` is the explicit operator boundary for a forced new base.

The segment (`OWLS`, format 3) has a 72-byte checksummed header followed by:

```text
bincode-encoded cold catalog metadata
32-byte directory entry per live ObjRef
contiguous OWLR record bytes
independently checksummed scalar-index blocks
concatenated checksummed OWLC typed row groups
```

Each directory entry stores the packed stable reference, record-region offset,
record length, and record checksum. Bincode is confined to checkpoint catalog
metadata; it is not the durable document or index representation. Each scalar
index block stores its schema/name identity, uniqueness flag, strictly ordered
typed keys, and strictly ordered stable-reference postings. Blocks carry their
own CRC32 in addition to the whole-segment checksum. The v3 header stores the
typed-region length as a 64-bit value; readers remain compatible with the
64-byte format-2 header, which has no typed region.

Runtime publication uses a streaming `OWLS` writer: it emits the header,
metadata, object directory, individual records, and index region sequentially;
patches the internal checksum; fsyncs; then computes the manifest checksum with
a bounded 64 KiB verification buffer. Runtime recovery verifies both checksums
as streams, reads the metadata and directory ranges, and reads records
sequentially one at a time. The old whole-slice codec remains only as a test
oracle, with a byte-for-byte parity test. Recovery initializes the catalog from
the decoded metadata and then restores each record through a callback, avoiding
an intermediate `CheckpointImage.records` collection.

Ordered-index checkpoint data follows the same bounded-I/O direction. Each
logical index is partitioned into contiguous key-range pages targeting 16 KiB;
an individual entry larger than the target occupies one overflow page. Every
page repeats the index identity, carries first/last chain flags, and has an
independent checksum. Full publication plans page ranges without materializing
encoded pages, then encodes and writes one page at a time. Recovery seeks to the
index region, verifies one page, and validates its entries before reading the next.
Legacy whole-index blocks decode as single-page chains, preserving format-2
read compatibility.

After recovery, a runtime ordered index retains only its authenticated page
descriptors and a reference to the immutable segment. Point lookups read the
single page whose first/last keys contain the target; range scans read only
intersecting pages. The mutable layer is a complete-postings overlay: the first
write to a key loads that key from the base, after which inserts, deletes, unique
checks, and tombstones are resolved in memory. Full checkpoint publication swaps
the base handle after manifest durability while preserving keys dirtied by
concurrent commits. Query paths clone the runtime index handle before file I/O,
so page reads do not hold the global storage-state mutex. A limited range over a
clean base stops after enough postings have been read; if mutable keys or
tombstones overlap the range, execution reads all intersecting pages before
merging so replacements cannot make an early stop incorrect.

Incremental recovery treats `OWLI` as the authoritative posting overlay. WAL
frames covered by the checkpoint still restore documents and index catalog
operations, but deliberately skip ordered-posting maintenance. Recovery clears
the mutable overlays on surviving base handles, installs complete postings and
tombstones from `OWLI`, and honors reset/delete operations against the
WAL-replayed catalog.

`OWLI` format 2 stores the cumulative overlay as ordered, target-16-KiB change
pages. Each page repeats its index identity and mode, carries first/last chain
markers, and has an independent checksum; an oversized key/posting change may
occupy a larger page. Publication first plans page counts and lengths, then
writes each encoded page directly to the file. It never constructs a serialized
root-sized byte buffer. Incremental publication performs an ordered two-way merge
between the prior root pages and the newly dirty keys. It retains only one input
page, one target-sized output page, and the new dirty-key delta, while preserving
patch, reset, and delete semantics. Recovery likewise validates and installs one
page at a time. Format-1 `OWLI` files remain readable as legacy single-page index
chains; merging a legacy root may temporarily load that legacy file once.

This authority does not weaken fail-closed recovery. A bounded audit streams the
final base-plus-overlay view page by page, checks every posting against its live
document and typed key, enforces unique cardinality, and requires the posting
count to cover every document exactly once. WAL-replayed index resets and deletes
must also appear in `OWLI`. Thus a checksummed but incomplete root fails open
without reconstructing full index maps.

The manifest (`OWLM`, format 2) is a fixed 64-byte checksummed record containing
its sequence, checkpoint LSN, selected slot, segment length/checksum, and
live-object count.
This sidecar layout is transitional and is not a claim that the final directory
or index design is complete.

## 6. Schema-compiled record format

Documents are not stored as maps and field names are not repeated in each row.
The schema compiler assigns stable field IDs and produces an encoder/decoder
plan. A mutable record contains:

```text
RecordHeader
  format_version
  schema_id
  schema_version
  object_id
  generation
  flags
  fixed_length
  variable_length
  checksum

null bitmap
fixed slots
variable-value directory
variable bytes
```

Fixed slots contain integers, fixed decimals, floats, booleans, timestamps,
`ObjRef`s, and offsets to variable values. Strings, bytes, lists, and embedded
objects live in the variable region. Projection reads should touch only selected
slots and their variable spans.

Append collections use the same logical values and identity. Their segments may
add row-group metadata, timestamp min/max values, dictionaries, null bitmaps,
and separately compressed large payload blocks. This optimization must not leak
into the general object contract.

### 6.1 Implemented `OWLR` format

Format version 1 is now the durable document representation inside WAL version
2. It is little-endian and consists of:

```text
80-byte header
null bitmap
16-byte fixed slot per compiled field
variable region
```

The header records the format version, compiled schema/collection ID, compiled
schema version, object ID and generation, document version, timestamps, region
lengths, external ID length, total record length, and a CRC32 checksum. The
checksum covers the header (with its checksum field zeroed) and every payload
byte, so identity corruption cannot redirect an otherwise-valid payload.

Fixed slots directly encode integers, floats, booleans, timestamps, and local
references. Strings, lists, embedded objects, and global references use checked
offset/length pairs into the variable region. Nested values use a versioned
tagged encoding with explicit length and nesting limits. Dynamic embedded object
keys remain data, but compiled top-level field names do not appear in records.

Compiled schema entries give fields stable 16-bit IDs. Schema versions preserve
IDs for surviving fields and allocate new IDs monotonically. Recovery retains
all referenced compiled versions, allowing old and new records to coexist.

Projection decoding is implemented in the codec: unrequested variable fields
are not decoded. The current `DocumentStore` API still materializes complete
documents; connecting projection requests from physical operators is part of
Milestone D.

## 7. Transaction and durability protocol

The initial concurrency model is a serialized commit coordinator. A batch is:

1. Validate schemas, reference targets, constraints, and expected versions.
2. Reserve object IDs and record/index space in memory.
3. Produce the complete set of data, directory, index, and catalog mutations.
4. Validate them against a private catalog overlay, including record decoding
   and every other fallible check; do not clone the object directory or records.
5. Encode one checksummed WAL batch with a contiguous LSN.
6. Append the batch and perform one durability sync for the commit group.
7. Apply the already-validated mutations and publish the committed LSN.
8. Acknowledge the caller.

No fallible logical work occurs after the durable batch and before publication.
If later physical application fails, recovery repeats the committed batch.

OQL `insert [...]` validates the complete document set, including same-batch
schema-level uniqueness, before calling `DocumentStore::put_documents`.
RustStore turns that call into one WAL batch, one LSN, and one sync; validation
or WAL failure publishes none of the documents. Compatibility stores retain a
sequential default until they implement an atomic batch boundary.

`GroupCommitter` is an opt-in synchronous coordinator for concurrent ingest
producers. The first request opens a bounded `max_delay` window and the worker
coalesces requests up to `max_documents` into one atomic WAL batch/LSN/sync.
Every successful caller in that cohort receives the shared durable LSN. If the
combined cohort has a logical validation conflict, it appended no frame and the
worker retries requests separately so the conflicting request does not reject
independent producers. Descendants should use explicit OQL/storage batches when
cross-request atomic visibility is not desired.

The registered `ingest_bench` creates a fresh RustStore per sample so setup and
unbounded WAL growth are excluded. On the local optimized 2026-07-22 run, 10
individually synced documents took about 40.8 ms versus 5.54 ms in one atomic
batch (7.4x); 100 took about 452 ms versus 8.11 ms (55.8x). These are local
filesystem durability measurements, not yet a cross-platform or RocksDB
comparison.

## 8. Recovery and checkpoints

Recovery:

1. Select the highest valid manifest copy.
2. Verify its referenced segment length and checksum, then load its catalogs,
   object directory, and records.
3. Replay complete, checksummed WAL batches after the checkpoint LSN, reading a
   surviving sealed generation before the active generation.
4. Ignore a torn final frame.
5. Reject corruption in a complete frame.
6. Rebuild any derived index whose persisted watermark trails the committed LSN.
7. Validate directory and collection invariants before opening for writes.

Checkpoint publication:

1. Under the serialized commit boundary, freeze only the committed LSN and
   maintenance counters.
2. Close and atomically rename the synced active WAL to the fixed sealed-WAL
   generation, create and sync a fresh active WAL, then release the commit
   boundary.
3. Outside the commit boundary, reconstruct the frozen state from the previous
   published checkpoint plus the immutable sealed WAL, then encode, write, and
   sync the inactive segment slot while new commits append to the fresh WAL.
4. Write and sync the matching inactive manifest copy, publishing the snapshot.
5. Remove the sealed WAL only after manifest publication. The active WAL is not
   truncated because it contains commits newer than the frozen checkpoint LSN.

`RustStore` schedules this publication automatically after a durable commit.
The default policy checkpoints after 10,000 commits or 64 MiB of WAL, whichever
comes first; when a finite storage-size limit is smaller, the WAL threshold is
reduced to half that limit to preserve headroom. Applications can replace or
disable these thresholds with `AutoCheckpointPolicy`.

Automatic maintenance is scheduled onto one store-owned worker after the
committed state and commit lock have been released. Its bounded one-message
queue coalesces bursts: the worker rechecks both thresholds instead of creating
one full snapshot per signal. A maintenance failure is exposed by
`maintenance_status()` and never changes an already durable write into an error
that a caller might retry. A successful checkpoint resets the commit counter,
records its metadata, and clears the previous maintenance error.

`wait_for_maintenance()` is an explicit barrier for administration, orderly
tests, and callers that need checkpoint completion. Ordinary writes do not wait
for it. Store shutdown drains a checkpoint signal already accepted by the
worker before joining it.

Checkpoint reconstruction, index snapshotting, encoding, and file I/O are
therefore concurrent with subsequent commits. The commit boundary covers only
LSN capture and WAL generation rotation; it does not walk live records or clone
index postings. If publication fails, the pending snapshot and sealed WAL remain
for retry while new commits remain durable in the active WAL. Recovery replays
the sealed generation before the active generation and publishes a consolidation
checkpoint before accepting new writes.

This bootstrap approach intentionally trades background read/replay work for a
short foreground pause. Each checkpoint currently reads the previous full
snapshot, replays the sealed WAL, and writes the next full snapshot. Incremental
copy-on-write record/index pages should eventually replace that full-snapshot
write amplification without reintroducing live-state copying on the commit path.

The first COW record-page layer is implemented in memory. The record directory
uses a fixed 256-way root whose children are immutable ordered maps, and record
payloads are immutable shared byte allocations. Updating or deleting an object
copies only its selected shard; a snapshot clones the fixed root pointers and
continues to observe the prior record versions. Checkpoint images share those
record allocations until final segment encoding rather than copying every byte
buffer into an intermediate image. This establishes snapshot ownership and
update isolation, but it does not yet make the on-disk segment incremental.

The bootstrap checkpoint layout has no unbounded generation set to garbage
collect. It reuses two fixed manifest and segment slots; creating the inactive
slot truncates its previous contents before writing the next snapshot. Thus
repeated checkpoints retain at most two complete generations plus the bounded
WAL. Explicit reachability reclamation can reduce this to the single
authoritative generation. Future variable-generation incremental segments can
reuse the same manifest-first reclamation rule.

If a crash occurs before manifest publication, the older checkpoint plus sealed
and active WAL generations are authoritative. If it occurs after publication
but before sealed-WAL reclamation, replay skips frames at or below the checkpoint
LSN and continues through the active generation. A torn inactive manifest is ignored.
A complete corrupt manifest, or corruption in the segment selected by the newest
valid manifest, fails closed instead of silently falling back to a checkpoint
whose required WAL may already have been removed.

Replay is idempotent. Recovery must be followed by a successful insert test so
allocator or ID state cannot silently overlap recovered data.

## 9. Index architecture

Index keys use a versioned, memcomparable typed encoding. The byte order must
match OQL value order for signed integers, decimals, timestamps, strings, nulls,
and composite values. Hashes may accelerate comparison but never substitute for
collision-safe equality.

Non-unique keys map to postings, not one overwritten object reference. Index
entries include the object identity and, where required, an MVCC visibility
field. Every authoritative index changes in the same commit batch as its object.
A derived index may lag only when it persists an explicit source-LSN watermark.

Initial useful Loggr indexes are:

- primary external ID
- `(application, occurred_at descending, event_id)`
- `(application, level, occurred_at descending, event_id)`
- `(application, fingerprint, occurred_at descending)`
- trace ID
- error fingerprint

The general engine also requires unique and ordered composite indexes for
financial, game, and commerce workloads.

### 9.1 Implemented typed-index nucleus

Typed key format version 1 is now implemented. A key starts with a format byte
and concatenates self-delimiting components. Within each supported type,
lexicographic byte order matches value order for null, boolean, signed integer,
finite/infinite float, UTC timestamp, UTF-8 string, local reference, and global
reference values. Signed numeric encodings use sign-aware big-endian transforms;
strings use an escaped terminator so composite boundaries are unambiguous.
Negative and positive zero have one float key and NaN is rejected. Lists and
objects are not ordered index keys.

The native ordered index maps each key to an ordered posting set of stable
`ObjRef`s. Insert, update, and delete maintenance occurs while applying the same
WAL batch as the object. Unique definitions and writes are checked before WAL
append, including multiple writes in one batch. Missing indexed fields currently
encode as null, and unique indexes therefore permit only one identical null
composite; SQL-style multiple-null semantics have not been adopted implicitly.

Checkpoint format 2 persists scalar index definitions and independently
checksummed ordered-posting blocks. Recovery restores these blocks directly,
rejects unknown/missing blocks, validates every posting against its `OWLR`
record, and verifies that every indexed live document has a posting before WAL
replay. This avoids rebuilding and sorting the index from scratch, but it is
still a full-snapshot format rather than incremental copy-on-write pages with
source-LSN roots.

`DocumentStore` now exposes an engine-neutral typed scalar-index range contract.
`IndexScanExec` uses it directly when the selected definition belongs to the
store: it translates equality and inclusive/exclusive range predicates into
typed bounds, obtains postings from the native ordered index, and resolves each
stable reference without scanning the collection. Composite scans support exact
keys, equality prefixes, and a range on the next component, including correct
exclusive bounds when trailing components exist. Dotted field paths and
intrinsic document fields such as `id` use the same key extraction as write-time
maintenance.

Native scans may push a row limit into the postings iterator and a top-level
field projection into `OWLR` decoding. Keyset cursors resume after the ordered
pair `(typed key, ObjRef)`. Cursor format 2 is versioned, index- and
direction-bound, checksummed, and URL-safe encoded. Forward scans order both
keys and duplicate postings ascending; reverse scans order both descending, so
neither direction repeats or skips equal-key rows across pages. Cursors remain
read-committed continuation tokens, not snapshot
cursors: concurrent committed inserts after the cursor may appear on a later
page until MVCC snapshot pinning exists. Explicitly injected index managers and
external compatibility catalogs retain the historical hashed path, but it is no
longer the default for RustStore indexes.

The physical planner now annotates native index scans with required decode
fields and safe limits. Projection requirements include fields consumed by
lower filters and sorts, not only the final result. A limit crosses a projection
and fully index-enforced predicates, but is blocked above a sort or any residual
condition that the chosen single/composite index does not encode. The executor
passes these annotations to `IndexScanExec`, so the storage iterator stops at
the requested posting and `OWLR` decodes only the required fields.

For a composite index, equality-constrained leading fields are constants and
the next key fields define result order. The planner removes an explicit sort
only when its top-level fields are a consecutive suffix of that ordered key and
all directions agree. It then selects forward or reverse index traversal, which
allows a following limit to become a true top-K postings scan. Unrelated,
computed, and mixed-direction sorts remain explicit and block early limit
pushdown.

Planner statistics can now load authoritative scalar definitions from
`DocumentStore`; selection uses the real persisted name and composite leading
field rather than synthesizing `idx_<collection>_<field>`. Physical-plan explain
output identifies native typed versus compatibility access. Runtime
`IndexScanExec` reports the actual backend, index, key fields, limit, projection,
cursor continuation, and rows loaded.

Ordinary OQL logical plans now select that access path at the actual
`Filter -> Scan` boundary. The executable read tree supports `SeqScan`,
`IndexScan`, `Filter`, `Project`, `Sort`, and `Limit`; schema-validated queries
composed only from safe top-level comparisons and simple projections/sorts run
through this iterator pipeline whether the selected access path is a native
index or a sequential scan. The filter remains above the storage scan as a
residual correctness check.

Sequential scans use the same late-materialization contract. The executor
collects fields required by predicates, ordering, and final projection; the
Rust store evaluates pushed predicates against borrowed object-directory rows
and clones only matching projected values. Document identity and system
metadata remain available without appearing in the user-field projection.
The storage predicate representation preserves nested `and`/`or` structure, so
non-indexed boolean filters are evaluated before materialization rather than
forcing full rows through the executor.
When a transaction has pending writes, the store deliberately declines this
fast path and the executor uses the established full scan plus overlay, keeping
read-your-writes semantics authoritative. The planner also declines committed
index access while such writes exist, because those writes have not reached
index maintenance yet. Scans with no pending overlay avoid rebuilding an
intermediate ID map.

Top-level `or` predicates whose branches are all supported by native scalar
indexes become an `IndexUnion`. Each branch keeps its own typed range and
projection; the union deduplicates overlapping document identities before
returning deterministic ID order. A residual filter is removed only when every
OR branch is structurally covered by its selected index. Mixed indexed and
unindexed OR predicates use the pushed sequential path.

When every union branch proves the same composite-index suffix order, the
planner also removes the explicit sort. A following global limit is pushed to
the union and independently to every branch: no branch can contribute more
than the global top-K, so at most `branch_count * K` candidates need decoding.
The union deduplicates, applies the proven order with stable identity tie
breaking, and truncates to K. Unordered unions retain full materialization and
deterministic identity order.

Sequential execution owns the vector returned by storage and drains documents
into execution chunks. It no longer clones every materialized row a second
time. Reset reloads the scan from its retained storage handle, preserving the
operator contract without retaining a duplicate document set.

RustStore advertises an explicit storage-owned scan capability. For safe
top-level scalar filters, the physical planner can replace
`Filter -> Project -> SeqScan` with `StorageScan`. Predicate fields and result
fields are separate: predicates inspect borrowed object-directory values while
only final result fields are cloned. A plain identity projection is removed;
aliases, literals, sort-only fields, and document system fields retain the
minimum required upper operator. Stores that do not advertise the capability
keep the ordinary residual filter/project pipeline. Transaction-local writes
continue to disable this planned path because the authoritative overlay must
run until commit.

Live state also maintains a per-collection `BTreeMap<DocumentId, ObjRef>`.
Put, update, and delete mutations change it atomically with the identity and
object directories; WAL replay and checkpoint hydration rebuild the same
derived structure. Integrity checking validates its cardinality and every
bidirectional identity mapping. Normal full, projected, and pushed-filter scans
walk this stable order directly. Only transaction overlays retain a temporary
ordered merge, eliminating the former gather-and-sort operation from ordinary
queries without adding the directory to the durable format.

Queries with reference navigation, complex expressions, aggregation, vector
search, or mutation continue through the established executor rather than
being partially translated. `execute_query_with_plan` reports either physical
access path; `explain_indexed_read` remains the narrower answer to whether a
native index was selected.

Sort direction parsing accepts both `.timestamp.desc` and `.timestamp desc`.
The former is normalized out of the tokenized field path, preventing `desc`
from being mistaken for a nested property.

The registered Criterion benchmark `index_scan_bench` compares two equivalent
OQL queries over RustStore: a 1%-selective `(level, sequence)` index answering
`sequence.desc limit 10` through reverse traversal, and an OR-shaped scan/sort
oracle that deliberately remains on the fallback. A local optimized run on
2026-07-22 measured approximately 43.7 us versus 1.62 ms at 1,000 documents
(about 37x), and 41.9 us versus 201 ms at 10,000 documents (about 4,790x, with
high scan variance). This is an in-process cached read-path
microbenchmark, not yet a RocksDB comparison or a write/durability benchmark.

The same benchmark also contains a selective non-indexed predicate over rows
with a 1 KiB unselected payload. On a local optimized run on 2026-07-24, the
safe late-materialized sequential path measured approximately 178 us at 1,000
documents and 3.7 ms at 10,000 documents. Moving the pushed scalar predicate
to a storage-owned borrowed-value representation then reduced the final path
to approximately 79.7 us and 1.26 ms. Before filter pushdown it measured
approximately 1.12 ms and 20.2 ms; the equivalent general full-document path
measured approximately 1.91 ms and 96.4 ms. These figures establish the
regression shape, not a cross-database performance claim.

For two overlapping OR branches on the same native scalar index, the same
10,000-document fixture measured approximately 369 us through `IndexUnion`.
The equivalent mixed indexed/unindexed OR scan measured 47.2 ms before boolean
predicate pushdown and 2.61 ms after it. The union was therefore about 7x
faster than the final scan path and about 128x faster than its former
full-materialization shape.

An ordered two-branch union spanning the fixture and returning top 10 measured
approximately 155 us at 10,000 documents. The branch row goal bounds decoding
to at most 20 candidates. In contrast, a broad projected sequential query over
all 10,000 rows measured approximately 22.9 ms even after eliminating the
executor's second document clone. That number is the current honest boundary:
the scan still performs row-oriented field lookup, row shaping, and ID sorting;
it is chunk-owned but not yet a vectorized or columnar scan.

After storage-owned filter/projection fusion, a same-process controlled run of
that broad shape measured approximately 31.8 ms fused versus 116.9 ms through
an otherwise equivalent capability-disabled wrapper (about 3.7x). Absolute
times on that run were slower than the earlier quiet-machine result, so the
ratio—not a cross-run absolute comparison—is the useful evidence. A selective
one-row shape was effectively neutral at approximately 1.93 ms in reversed
order controls; fusion planning itself measured about 10.8 us versus 8.75 us.
Broad scans still require a typed batch representation to remove row
`HashMap` lookup; maintained scan order now removes per-query ID sorting.

After introducing the maintained ordered collection directory, the 10,000-row
broad projected scan measured approximately 9.17 ms on the same Criterion
shape, versus the earlier 22.9 ms result before storage-owned fusion and
ordered traversal. The exact cross-run ratio is environment-sensitive, but the
algorithmic change is fixed: ordinary scans no longer allocate and sort an ID
list. Remaining time is dominated by string-keyed row field access and
projected `Document` construction.

The durable record codec also exposes schema-version-specific numeric field
slots through a borrowed `RecordView`. Scalar strings borrow their encoded
bytes, fixed-width values are read directly, and projections decode resolved
slot ordinals without a per-row field-name hash. Owl authenticates records when
they enter live state or checkpoint storage, so this internal view retains
region/schema bounds checks without redundantly checksumming the whole record
on every predicate.

That representation is not forced onto the current hot scan. Controlled
10,000-row measurements showed a selective resident slot scan at approximately
3.59 ms even after removing per-row `Arc` clones, while the already-resident
decoded directory answered the same shape in approximately 1.00 ms after a
checkpoint. Parsing record regions was more expensive than one borrowed
`HashMap` lookup. RustStore therefore selects the cheapest resident
representation: decoded rows today, compiled slots when a future
memory-bounded mode evicts decoded rows. The rejected 14.55 ms checksum-heavy
prototype is deliberately not the production path.

Checkpoint-backed multi-record projection no longer performs one base-segment
seek and read per object. The base directory first resolves the requested
`ObjRef`s, reads their contiguous physical span once, verifies each record, and
returns results in caller order. A deliberately forced checkpoint-record scan
measured about 10.94 ms selective and 21.82 ms broad before residency-aware
selection; choosing the decoded live directory restored approximately 1.00 ms
and 10.26 ms respectively. The multi-read remains active for projected graph
navigation, where record bytes are actually required.

The broad result is now an output-shaping boundary rather than a row-selection
boundary: returning 10,000 public `Document` values still allocates 10,000
document IDs, schemas, field maps, and selected strings. Removing that cost
requires a typed/columnar query-result batch retained through the executor and
materialized only at the API boundary; changing predicate lookup alone cannot
remove it. A final same-build production-path run measured approximately
1.61 ms for a selective one-row projection and 9.91 ms for the 10,000-row broad
projection; the latter was statistically unchanged from its prior baseline.

The first delayed-materialization slice now changes that boundary. Live
documents in the object directory are immutable `Arc<Document>` values.
Storage-owned scans evaluate predicates against borrowed rows and return
1,024-row projected batches containing shared row ownership plus a projection
descriptor. `DataChunk` can carry these batches without constructing field
maps; `Limit` truncates them directly, while row-only operators materialize as
a compatibility boundary. The collector reports materialization separately
from physical execution, and Loggr exposes total, execution, materialization,
and JSON document-conversion milliseconds.

On the same 10,000-row fixture, storage selection plus deferred projection fell
from approximately 3.70 ms to 1.18 ms. The selective end-to-end query measured
approximately 0.96 ms, and the broad end-to-end query fell from approximately
9.91 ms to 8.41 ms. Constructing the 10,000 public `Document` maps is now
isolated at approximately 4.03 ms. This is an intentionally transitional
selection-vector representation: it removes eager value copies and proves the
executor boundary, but the underlying shared rows remain `HashMap` documents.
The next conversions are sort/offset over deferred rows, direct result
serialization, and ultimately typed storage columns.

The generic document result is no longer Owl's canonical broad-read boundary.
`execute_query_batches` retains projected `DataChunk`s through executor close
and returns their ownership directly to a streaming or serialization consumer;
`execute_query` remains the compatibility adapter for callers that explicitly
need `Vec<Document>`. On the same final optimized build, the complete planned
10,000-row broad batch query measured approximately 0.97 ms. The document
compatibility path measured approximately 7.42 ms. These are deliberately
separate benchmark names: the difference is public row materialization, not
storage or predicate execution, and must not be attributed to the engine.

Loggr consumes this boundary directly. Its OQL endpoint serializes borrowed
projected row views into the HTTP JSON byte buffer, including nested clean
values, without first constructing public documents or a `serde_json::Value`
tree. A release-build run over 10,000 realistically seeded Loggr events put the
indexed, descending, seven-field, 200-row workbench query at approximately
2.36–2.49 ms before socket transfer: approximately 2.00–2.10 ms execution and
0.26–0.28 ms result serialization. An intentionally unbounded 10,000-row,
2.07-MB JSON response spent approximately 3.2–4.0 ms in warmed execution and
10–11 ms in JSON serialization. That large-response cost is reported
separately rather than mislabelled as database execution.

The immutable typed-segment nucleus now replaces shared `Document` ownership
for fully covered checkpoint scans. `OWLC` row groups cap at 65,536 rows,
preserve missing versus null with separate bitmaps, store strings as offsets
plus bytes, preserve nanosecond timestamps, and carry integer min/max
statistics. A segment-wide predicate may classify a group as all, none, or
row-dependent. Projected batches retain an `Arc` to the row group plus row
indices; Loggr serializes typed scalar references without string copies.

The first bounded 100K × 64-byte measurement produced 0.476 ms median lazy
batch assembly and 5.224 ms when the harness additionally walked every row and
hashed all 1.89 MB of selected strings. The checkpoint occupied 64.19 MiB
across retained crash-safe files, or 7.01× logical size. That space ratio is a
known migration cost because `OWLR` rows and `OWLC` columns are duplicated.
That was a migration measurement, not the current layout.

OWLC v2 now marks lossless typed row groups explicitly authoritative. Full
checkpoints omit OWLR records for those rows; unsupported nested, list, vector,
global-reference, and non-current schema rows remain exact OWLR overflow rows.
Large values and string-byte blocks use LZ4 only when compression wins, with
checksums over stored frames and bounded decompression.

On the identical 100K × 64-byte workload, retained crash-safe storage fell from
64.19 MiB / 7.01× logical to 14.09 MiB / 1.54×. Median batch assembly was
0.470 ms and the all-selected-byte scan was 5.187 ms (19.28M rows/s), so the
space improvement did not regress the scan path. Both alternating checkpoint
generations are included in the 14.09 MiB total.

Recovery now installs authoritative objects as `(segment, row)` locations.
Point reads materialize and cache one requested `Document`; typed scans remain
column-native. Reverse adjacency is persisted as a hidden target-keyed ordered
index: `(target ObjRef, field path)` maps to ordered source `ObjRef` postings.
Its pages use the same independent checksums and lazy `FileIndexBase` cache as
ordinary scalar indexes. Recovery installs page descriptors only; one reverse
lookup reads only pages intersecting that target prefix and resolves IDs for
returned sources.

Post-checkpoint puts and deletes update only changed edge keys in the B-tree
overlay. WAL replay maintains this overlay even when ordinary scalar indexes
are restored from a cumulative OWLI root. A full checkpoint publishes the
merged reverse base. Older checkpoints without this hidden root remain
readable through one legacy derivation and are upgraded by the next full
checkpoint.

This fixes bounded document residency, but not startup latency: the measured
reopen was 173.75 ms versus 175.71 ms before laziness, within run variance.
External-ID, ordered-ID, and `ObjRef` location maps are still eagerly built in
memory. A persisted paged identity directory remains necessary for faster
startup and billion-row identity scaling; the current map is a transitional
runtime directory, not the final physical design.

Ordinary reopen and explicit integrity audit are now separate operations.
OWLS v4 gives the fixed header, schema/catalog metadata, and object-record
directory one bounded root checksum. Reopen validates the manifest, that root,
region bounds, and then the checksummed records/pages/typed segments it
decodes. It no longer performs an additional whole-file checksum stream before
doing that work.
`check_integrity`, live integrity verification, and safe reclamation retain the
complete immutable-file audit. This is a semantic boundary, not a permission
to ignore corruption: lazy pages must verify their local checksum before use.

OWLC v3 also exposes a fixed 64-byte row-group descriptor containing authority,
collection/schema identity, row and column counts, identity-prefix length, and
encoded length. Its compact identity prefix (object references, versions,
timestamps, ID offsets, and UTF-8 ID bytes) has an independent checksum, so it
can be loaded and trusted without reading the column payload. It retains IDs in
one byte array rather than allocating one `String` per row. Checkpoint
discovery reads one descriptor per row group and can skip its body. The current
recovery path still follows discovery by decoding every typed body and
rebuilding the three identity maps, so this is the foundation for file-backed
lazy segments rather than a claimed micro-open.

Fresh 100K × 64-byte runs after the open/audit and bounded-root changes reopened
in 154.85–179.02 ms. The old 173.75 ms figure sits inside that noisy range:
there is no material startup win. This isolates the remaining dominant work:
typed-body decode plus per-row `String`, hash-directory, and ordered-tree
construction. Those structures must be replaced by the persisted
identity/locator root; optimizing their rebuild loop is explicitly not the
plan.

Recovery now installs file-backed typed-segment handles. OWLC descriptors and
their independently checksummed identity prefixes are read at open, while
column payloads remain on disk until the first point materialization or typed
scan. The handle verifies and caches the complete row group on first touch.
Schemas without reference fields no longer load typed columns while rebuilding
the reverse-reference directory.

The first measurement of this boundary is intentionally not presented as a
latency win. At 100K rows, reopen reported zero of two typed segments loaded,
but still took 189.38 ms because it read 7,000,008 identity-prefix bytes and
rebuilt 100,000 entries in each legacy base directory. The first scan paid
19.00 ms to load/decompress both segments; subsequent batch assembly returned
to 0.481 ms and selected-value traversal to 5.205 ms. This proves lazy column
ownership works and that the persisted external-ID/fence directory—not further
column-decoder tuning—is the next reopen task.

The first two base identity directories have now been removed. Authoritative
rows are not inserted into the external-ID hash or collection-order B-tree.
Each collection retains only one first/last ID fence per immutable segment;
point lookup selects a segment by fence and binary-searches its persisted UTF-8
ID bytes. Ordered scans merge immutable segment order with the existing
WAL/overflow B-tree, which is empty for a clean fully typed base.

At 100K rows this made both `external_id_hash_rows` and
`ordered_id_tree_rows` zero. Reopen fell from 189.38 ms to 182.73 ms after
removing the hash, then to 152.12 ms after removing the ordered tree. Warm
batch assembly remained 0.466 ms.

The `ObjRef` directory is no longer a hash table either. It is a dense
collection/object-ID-indexed vector, preserving O(1) graph navigation. Its
original slots were packed `u64` values: zero is absent and every present value
encodes a segment index plus row. This avoided the substantially wider Rust
`Option<(usize, usize)>` representation. OWLL v3 subsequently reduced fresh
slots to a checked `u32`; the 65,536-row group cap makes 16-bit segment and row
components sufficient for the current single-generation format. Older 64-bit
leaves remain readable.

OWLC v4 also splits the mixed identity prefix. Object references, ID offsets,
and ID bytes form an independently checksummed lookup leaf; versions and
timestamps remain cold with the full row-group body. On the 100K × 64-byte
case this reduced open-time typed bytes from 7,000,008 to 3,800,008 and reopen
from 147.11 ms to 88.11 ms. Packing the locator slots then measured 85.05 ms,
with zero decoded typed segments and 0.295 ms median warm batch assembly.

OWLS v5 persists the packed locator in an independently checksummed OWLL leaf.
Ordinary recovery installs that immutable leaf directly and keeps only
post-checkpoint insert/delete overrides in memory. Older checkpoint versions
remain readable through the previous rebuild path. The 100K controlled case
reopened in 25.26 ms, down from 85.05 ms.

OWLL v2 adds first/last external-ID fences for every authoritative segment.
Ordinary open installs this small authenticated root while ID offsets and ID
bytes remain file-backed in OWLC. A point lookup loads only its routed segment
leaf; an ordered scan loads leaves as it advances. Explicit audit still walks
and validates every leaf.

An empty reverse-reference target set now resolves immediately instead of
constructing a complete object identity map. Without this guard, schemas with
no reference edges accidentally forced all lazy ID leaves into memory.

The controlled results after that boundary were 0.97 ms reopen at 100K and
11.93 ms at 1M, with zero resident ID-leaf bytes and zero decoded column
segments at both scales. First-scan time moved to 18.68 ms and 207.80 ms
respectively; median warm batch assembly was 0.469 ms and 5.010 ms. Reopen is
therefore near, but slightly outside, the 12× scaling gate at roughly 12.3×.

Post-base mutations now use an ID-ordered overlay whose entries are live
insert/update references or delete tombstones. The scan merge advances directly
through segment IDs and this bounded overlay; unchanged rows remain typed and
changed rows alone use decoded predicates and shared-row output. At 100K rows
with 100 post-base updates, median batch assembly was 0.946 ms and the
all-selected-byte scan was 5.720 ms, versus 0.476 ms and 5.224 ms with an empty
delta. Recovery rebuilds the same overlay from WAL replay, while a full
checkpoint installs a fresh typed base and clears it when publication did not
race a newer commit.

Full compaction no longer reconstructs the frozen base plus sealed WAL as a
complete decoded `State`. WAL replay coalesces authenticated encoded records
and tombstones; the compactor merges those changes with lazy base identities
in external-ID order and decodes at most one 65,536-row group while rebuilding
typed segments, overflow records, scalar indexes and reverse adjacency. This
removed a measured ~121-second synchronous destructor tail at one million
sources: the controlled initial checkpoint fell from 156.9 to 12.3 seconds,
and a subsequent full checkpoint over the persisted base plus 100 mutations
took 4.68 seconds.

The design deliberately does not detach destruction to a background thread.
Compaction is now strictly bounded on its WAL-change side: changes coalesce in
a 16-MiB ordered buffer, spill to sorted native runs, and enter checkpoint
construction through a heap-based k-way merge cursor. Runs contain versioned
headers and independently checksummed, 256-KiB LZ4 blocks; entries use a
compact Owl-owned encoding for identity, object reference, mutation ordinal
and optional record bytes. The persisted base is walked by an iterator-native
identity cursor, so neither its IDs nor its documents are copied into a
whole-database temporary collection. A collection-partitioned bitset marks
base object slots touched by the WAL. Run selection uses a minimum heap, and
more than 32 runs are consolidated through bounded-fan-in passes; reader block
memory and open descriptors are consequently capped rather than proportional
to database size.

Temporary runs are operationally owned data, not anonymous scratch files.
Before a prospective run is created, Owl appends and syncs a fixed-size,
checksummed count record to a small manifest journal for a database-derived,
deterministic run namespace. Manifest write work is constant per run, a torn
final journal record is safe to ignore, and recovery never trusts an arbitrary
cleanup path from disk. Completion and error unwinding remove the set; a later
open removes a set left by process interruption. Each block has authenticated
dimensions and payload; malformed entries, decompression failures, truncation
and checksum mismatches fail closed.

At one million sources the buffer peaked at 16.00 MiB while 22 runs held
59.47 MiB of compressed changes. The initial checkpoint measured 12.806
seconds versus 12.313 seconds for the former unbounded encoded overlay; the
persisted base plus 100 mutations remained 4.671 seconds. A dedicated initial
builder is deliberately deferred: it would introduce a second correctness
path to recover roughly 4% in this sample, while the general path already uses
less scratch than the final 98.73-MiB database.

### Billion-row storage boundary

The 10M experiment invalidated several mechanisms that were acceptable at
1M. Owl does not claim billion-row readiness while any of these remain on the
ordinary maintenance path:

- a full checkpoint owns encoded bytes for every typed row and rewrites the
  complete historical base;
- even OWLL v3 uses one dense four-byte location slot per allocated object ID
  (approximately 3.73 GiB for one billion slots before allocator and page-cache
  overhead);
- ingestion retains a general `Document`, external-ID hash entry, ordered-ID
  entry, schema-version entry, and compiled record for every uncheckpointed
  row;
- the fixed two-segment layout retains up to two complete database generations;
- reverse-result shaping resolves scattered source IDs after reading postings.

The required replacement is a partitioned immutable-generation design, not a
larger cache:

1. Commits enter bounded typed mutable row groups plus a small overflow arena.
   Typed scalar/reference fields are column vectors from the start; compatible
   writes do not first become a forest of general Rust maps and strings.
2. A flush publishes only those row groups, their local ID/`ObjRef` locator
   pages, and their scalar/reverse index pages. Historical generations are
   referenced by the new manifest and are not copied.
3. Compaction is partitioned by collection and ID/object range. It merges a
   bounded set of input generations into one output generation and atomically
   replaces only those manifest references.
4. The locator becomes an authenticated paged structure with adaptive dense,
   sparse, and monotone encodings. Its root and page fences are resident; slot
   pages are read and cached on demand. No open-time allocation is proportional
   to total rows.
5. Query operators consume encoded batches and selection vectors across
   generations. A row becomes a `Document` only at the API/materialization
   boundary.

The generation set must be bounded: a flush-level cap prevents unbounded read
amplification, compaction has explicit byte and descriptor budgets, and
manifest-first reachability governs reclamation. Full database compaction is an
operator request or repair tool, not the implementation of `checkpoint()`.

The first immutable-generation slice is implemented. `checkpoint()` retains
the immutable OWLS base and writes only one new WAL generation, one paged local
record root containing that generation's changed compiled records, one paged
`OWLI` root containing its scalar and reverse-reference index changes, and the
small checksummed `OWGR` descriptor root. Each descriptor authenticates the WAL
by length and whole-file checksum, the record file by length and its
authenticated root directory, and the index file by length and checksum;
individual record and index pages carry their own checksums. Normal reopen does
not scan record payloads, while an explicit integrity audit reads and validates
every page. Point and batch reads probe the
newest generation record root first, then older generations and finally the
base, so a stale base row cannot win and clean compiled records can be evicted.
Recovery authenticates every referenced file, validates each local root's LSN
bounds against its WAL descriptor, and replays generations in LSN order.
Catalog and identity mutations still come from the WAL, but scalar and
reverse-reference postings are installed from local OWLI patches instead of
being recomputed from document fields. `compact()` folds the bounded set into a
new base and reclaims all generation files only after new
manifest authority is durable. Reopen also removes the deterministically named
next-sequence companion files that can be left before manifest publication.

This is deliberately a vertical durability slice rather than the final data
generation. Compiled record bytes and index patches for uncompacted changes are
now file-backed, but recovery still WAL-replays every generation and
materializes general `Document` maps for identity, update, and delete handling.
The next format step is generation-local typed row groups and identity locators,
so recovery and scans can route lazily without reconstructing changed documents.

The initial single-node acceptance envelope is deliberately concrete:

- resident storage metadata at open is bounded by catalogs, generation/page
  fences, and a configurable cache—not row count;
- write buffers and compaction scratch have hard byte ceilings and backpressure;
- point lookup touches at most one mutable overlay plus a bounded number of
  routed immutable pages;
- scans allocate by input/output batch size, never by collection cardinality;
- a durability checkpoint writes bytes proportional to changes;
- crash recovery replays only bounded WAL plus manifest-selected immutable
  generations;
- physical high-water space is measured during flush, compaction, interrupted
  publication, and reclamation;
- 1M, 10M, 100M, and 1B reports include rows, logical/physical bytes, peak RSS,
  page-cache bytes, generation count, read/write amplification, reopen latency,
  and cleanup verification.

The first corrective checkpoint changes are implemented. Publication now
installs the already-authenticated segment catalog as file-backed lazy handles
instead of decoding and validating every row again. All logical typed segments
share one positional-read file handle, avoiding both descriptor growth and a
shared seek lock. Installation retires only resident rows covered by the new
base and can structurally swap complete resident directories. Segment format 6
uses its streaming internal checksum as the manifest checksum, eliminating the
publisher's redundant full-file reread while preserving full streaming
integrity audit. OWLL v3 halves fresh locator slots from eight to four bytes;
this is a material size correction, but authenticated paging is still required
to remove row-proportional open and construction memory.

OWLL v4 is the first paged locator format. Fresh leaves divide each collection
into 4,096-slot pages; every page carries its own checksum while the root
checksum covers collection routing and ID fences independently of page
payloads. Locator lookup is now fallible throughout the storage engine, so a
corrupt or unreadable page cannot be misreported as an absent graph object.
The filesystem abstraction exposes cursor-independent positional reads and
writes for shared immutable handles and bounded spool construction. Fresh
publication and recovery install only the file-backed root and fences. A point
lookup reads and verifies one page, and a 64-page cache caps locator payload
residency at approximately 1 MiB regardless of row count. Legacy v1-v3 leaves
remain eagerly owned because their whole-leaf checksum has no independently
authenticated page boundary.

The locator publisher now uses that bounded builder. It accumulates at most
1,048,576 fixed ten-byte `(collection, object ID, packed location)` entries,
sorts and spills them, consolidates with fan-in capped at 32, and emits one
4,096-slot authenticated page at a time through a 256-KiB run-write buffer.
The final page spool is streamed into OWLS and removed by an ownership guard.
A forced multi-run test requires its bytes to equal the in-memory OWLL v4 codec
exactly. Locator-specific heap use is therefore bounded by one sort chunk, one
merge head per input, routing/fence metadata, and one output page—not total
row count.

Normal success and error unwinding reclaim run/page artifacts. Before creating
any spool, the publisher durably records the sequence and planned initial-run
count in a small checksummed journal beside the destination segment. Every
possible merge path is derivable from those two values. Reopen therefore
reclaims interrupted initial runs, merge outputs, and page spools without a
directory scan or row-proportional recovery metadata, then durably removes the
journal.

On the controlled one-million-source reference workload, publishing the
98.73-MiB segment took 0.895–1.126 seconds after this change. Reinstalling a
base after 100 mutations took 5.6–7.9 ms total, with 0.32–0.60 ms spent
retiring the changed resident rows. The remaining ~4.23-second post-mutation
checkpoint is dominated by rebuilding the complete image, which confirms that
partitioned generations—not further install-loop tuning—are the next
architectural task. Initial ingestion still paid about 5.07 seconds destroying
one million heavyweight resident documents after an O(1) directory exchange;
typed mutable row groups are required to remove that work rather than merely
move it.

## 10. Graph-specific advantage

Owl's graph behavior is built from ordinary objects, stable references, and
indexes—not physical pointers embedded in mapped files.

- Forward traversal resolves an `ObjRef` through the object directory.
- Persisted reverse-reference pages permit target-to-source traversal without
  rebuilding or hydrating the global edge set.
- Current index pages expose only an authenticated routing prefix at open:
  identity, first/last key fences and posting count. Entry/posting bytes retain
  their independent page checksum and are read only when a routed lookup
  touches the page. Older checkpoint pages use the compatible eager decoder.
- The planner knows reference direction, uniqueness, cardinality, and indexed
  fields and may choose either end of a path.
- Projection decoding loads only fields used by a path and its result.
- Stable identity permits future graph-aware compaction and locality hints.

For a path such as `orders.where(.customer.country == "NG")`, the planner may
start from indexed customers and traverse the reverse `order.customer` index
instead of scanning orders. That is a meaningful advantage over application-
managed KV keys. A claim of universally faster pointer chasing is not.

## 11. Required invariants

1. An acknowledged batch survives restart.
2. A failed or incomplete batch is invisible.
3. All mutations in a batch become visible together.
4. WAL LSNs are contiguous and replay is idempotent.
5. A complete corrupt frame is an error; it is never returned as missing data.
6. Allocated record regions never overlap.
7. Restart followed by insertion cannot overwrite recovered data.
8. Object references remain valid across updates and compaction.
9. A generation mismatch never resolves to another object.
10. Authoritative indexes and object data publish at the same LSN.
11. Every index result equals a scan-based oracle.
12. WAL growth is bounded by checkpoints.
13. Retention and compaction reclaim physical space.
14. Disk-full, short-write, sync-failure, and torn-write behavior is tested.

## 12. Implementation sequence

### Milestone A — Rust ownership and WAL nucleus

- remove the Zig tree, crate dependency, FFI adapters, and native metrics bridge
- introduce stable `ObjRef`
- add checksummed, versioned WAL batches and contiguous LSN recovery
- implement Rust object directory and atomic batch publication
- preserve schema and index catalogs in the batch stream
- establish crash/reopen tests

### Milestone B — Record segments and checkpoints

- [implemented] versioned schema compiler and record codec
- [implemented bootstrap] full-snapshot record segment and persisted directory
- [implemented in-memory foundation] sharded COW record directory and shared record blobs
- [implemented] manifest-referenced cumulative COW record roots with tombstones
- [implemented] double-manifest checkpoint publication
- [implemented] backward-compatible delta-aware manifest v2 and WAL-delta recovery
- [implemented] WAL truncation and checkpoint-bounded recovery
- [implemented bootstrap] coalescing background WAL/commit checkpoint scheduling
- [implemented bootstrap] bounded two-slot snapshot reclamation
- incremental mutable append-only data segments
- paged persisted object directories
- [implemented bootstrap] WAL rotation concurrent with checkpoint publication
- [implemented bootstrap] off-lock snapshot reconstruction from checkpoint + sealed WAL
- [implemented bootstrap] cumulative WAL-delta publication and bounded full compaction
- [implemented] spillable checksummed compaction runs, bitset overrides, iterator-native base merge, and crash cleanup
- [implemented] page-addressable base-plus-overlay COW record root format
- [implemented] file-backed lazy overlay and base-segment point-read handles
- [implemented] eviction of clean compiled bytes with lazy-delete tombstones
- [implemented foundation] native base-plus-overlay ordered-index COW root
- [implemented] streaming `OWLS` file decode/encode without whole-segment blobs
- [implemented] callback-based record restoration without `CheckpointImage.records`
- [implemented] streamed publication and recovery of independent index blocks
- [implemented] fixed-target key-range pages for full-checkpoint index blocks
- [implemented] file-backed index point/range reads with a complete-key overlay
- [implemented] `OWLI`-authoritative recovery without WAL posting maintenance
- [implemented] bounded streaming final-index consistency audit
- [implemented] paged, independently checksummed `OWLI` publication and recovery
- [implemented] streaming cumulative-root merge without materialized prior changes
- [implemented] explicit manifest-first reachability reclamation
- [implemented] offline/live integrity checker with semantic index/reference audit

### Milestone C — Typed indexes and constraints

- [implemented] memcomparable typed key codec
- [implemented checkpoint blocks] persisted ordered and unique indexes
- [implemented] non-unique postings
- [implemented] atomic unique constraint validation
- [implemented foundation] incremental copy-on-write index roots
- paged copy-on-write index nodes
- [implemented] target-keyed, paged reverse-reference base plus mutation overlay
- [implemented foundation] deterministic mutation/index/recovery differential test

### Milestone D — Read path and query integration

- [implemented safe indexed subset] parser-to-physical-operator pipeline for
  schema-validated `where`/`select`/`sort`/`limit` reads
- [implemented safe scan subset] storage-local predicate evaluation and late
  materialization for non-indexed top-level reads
- [implemented safe indexed subset] deduplicating native index unions for
  fully indexable top-level OR branches
- [implemented safe indexed subset] composite-order-aware bounded top-K index
  unions
- [implemented] move-owned sequential execution chunks without a second row
  clone
- [implemented safe scan subset] capability-gated `StorageScan` fusion with
  separate borrowed predicate evaluation and final-field materialization
- [implemented] maintained per-collection ordered row directory with
  recovery/integrity validation and sort-free ordinary scans
- one pipeline for the remaining read and mutation operations
- [implemented safe indexed subset] physical-plan projection and limit pushdown
- [implemented safe indexed subset] index-order-aware sort removal and
  ascending/descending composite top-K scans
- residual-filter removal after complete predicate pushdown
- [implemented nucleus] direct typed equality/range index execution
- [implemented] composite prefix/range execution, early limit, and keyset cursors
- [implemented nucleus] planned and runtime access-path reporting
- snapshot-pinned cursors and reference traversal
- typed parameters and timestamps

### Milestone E — Append policy and Loggr

The application boundary and acceptance workload are specified in
[`LOGGR_PRODUCT_AND_ARCHITECTURE.md`](LOGGR_PRODUCT_AND_ARCHITECTURE.md).

- [implemented nucleus] atomic OQL/storage batch ingest and opt-in bounded
  concurrent group commit
- timestamp zone maps and segment pruning
- compressed message, stack, and attribute blocks
- retention by segment
- text postings and fingerprint grouping

### Milestone F — hardening and comparison

- deterministic failure injection and restart soak tests
- fuzz record/WAL/index decoders
- format compatibility fixtures
- Log, Ledger, Game, and Commerce benchmark profiles
- fair RocksDB comparison with equal durability, batching, cache, and compression

## 13. Review gates

No milestone is complete solely because its happy-path tests pass. Each storage
format change must include compatibility/version handling, corruption tests, and
a statement of its crash atomicity. Performance claims require a reproducible
benchmark and must state the durability configuration used.

Scale results and physical-size accounting are maintained in
[`SCALE_BENCHMARKS.md`](SCALE_BENCHMARKS.md). A 10,000-row latency is not an
acceptance result for a scan-path change; the relevant review gate is the
throughput slope across record counts and widths, including checkpoint/reopen
and total temporary physical bytes.
