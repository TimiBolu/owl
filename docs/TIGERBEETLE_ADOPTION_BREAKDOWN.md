# TigerBeetle-Inspired Low-Level Adoption Breakdown for Owl

This plan is scoped to preserve Owl's strengths:
- Zig arena object-graph layout
- O(1) pointer navigation
- low-latency hot path

## Already adopted

1. WAL framed records with checksum (plus legacy reader)
2. Torn-tail tolerant WAL replay
3. Arena bootstrap recovery from WAL
4. Superblock copyset sidecar with startup quorum selection (best-effort writes, amortized)
5. Startup policy mode:
   - `availability` (default)
   - `strict` (quorum-required)

## Next candidates (ordered)

### A. Metadata integrity and startup safety

1. Superblock chain validation
- Add parent hash-chain checks between selected sequence and previous sequence.
- Value: detect forked/partial metadata histories.
- Perf impact: none on read/write hot path, startup only.

2. Metadata write quorum verification
- After writing copyset, re-read minimal quorum and verify checksum/sequence.
- Value: catches misdirected/torn writes earlier.
- Perf impact: metadata checkpoint only.

### B. WAL correctness and recovery semantics

4. WAL LSN + commit marker semantics
- Add monotonic LSN and explicit commit bit/record.
- Replay only committed records.
- Value: clearer prefix-durability contract.
- Perf impact: tiny write amplification on WAL only.

5. Per-record operation class checks
- Validate shape/content of replayed records (schema/doc invariants).
- Value: stronger corruption containment.
- Perf impact: recovery only.

6. WAL segment rolling + checkpoints
- Split WAL into segments; after durable checkpoint, mark segments reclaimable.
- Value: bounded startup replay and disk growth.
- Perf impact: low; occasional segment rotations.

### C. Background integrity (scrubber-style)

7. Incremental arena scrubber
- Idle-time scan of object blobs, verify blob header checksum and decodeability.
- Value: detect latent corruption before user query touches it.
- Perf impact: background I/O; tunable budget.

8. Quarantine list for bad object refs
- Maintain set of corrupted objrefs to skip fast and report.
- Value: prevents repeated expensive failures and improves UX.
- Perf impact: minimal (hash lookup).

9. Periodic metadata scrub
- Verify superblock copies and WAL segment headers in background.
- Value: early warning for storage issues.
- Perf impact: negligible if rate-limited.

### D. Repair and durability boundaries

10. Durable boundary API (checkpoint)
- Explicit `checkpoint()` that atomically seals WAL segment + superblock snapshot.
- Value: operator-visible durability point akin to TB checkpoint semantics.
- Perf impact: only at checkpoint trigger.

11. Recovery report artifact
- Emit structured report (replayed ops, truncated tail, repaired arena, corrupt copies).
- Value: observability + incident debugging.
- Perf impact: startup only.

12. Fault injection harness expansion
- Systematic tests for torn writes, partial sectors, misordered writes, stale copies.
- Value: confidence under power-loss chaos.
- Perf impact: test-only.

## What not to adopt (or not yet)

1. Full replica-level VSR repair protocol
- Not needed for current single-node Owl Explorer use case.
- Revisit only when distributed replication lands.

2. Heavy multi-zone storage architecture
- Owl's object-graph arena is a different model; keep architecture minimal.

## Performance guardrails to keep Owl fast

1. Never add synchronous verification on query/read path.
2. Keep all integrity checks on startup, checkpoint, or background budget.
3. Batch metadata writes (already amortized).
4. Keep arena pointer navigation and index lookups untouched.
5. Add microbench CI for put/get/scan p95 before each durability feature merge.
