# Owl Vector Production Checklist

This checklist tracks what is required to move Owl vectors from baseline to production-grade capability.

## Current Baseline (Already Implemented)

- [x] OQL `nearest` syntax and parser support
  - `collection.nearest { field, query, metric, k, ... }`
- [x] Vector type validation in schema-aware parsing/execution
  - field type is vector
  - query vector numeric
  - dimension matching checks
- [x] Exact top-k compute path delegated to Zig
  - Zig C API `owl_vector_topk_f32`
  - Rust bindings + executor integration
- [x] `_score` added to nearest results
- [x] Unit tests for parser and executor nearest paths

---

## P0: Production Core for Vectors (Must Have)

### 1) Durable vector index lifecycle
- [ ] Persisted vector index metadata per `(db, collection, field)`
- [ ] WAL-coupled index mutation log records for insert/update/delete
- [ ] Recovery/replay logic for index state on restart
- [ ] Resumeable rebuild markers/checkpoints

Acceptance tests:
- Crash during index build -> restart -> rebuild resumes and completes.
- Crash during write with vector mutation -> no index/document mismatch after recovery.
- Full restart preserves index metadata and state exactly.

### 2) ANN query path
- [ ] Implement ANN index backend (HNSW first)
- [ ] Query path supports `num_candidates`
- [ ] Exact fallback path for correctness safety
- [ ] Configurable per-index metric consistency

Acceptance tests:
- Recall@10 meets target threshold on standard dataset.
- ANN latency p95 significantly below exact scan for large collections.
- ANN and exact produce consistent top-k ordering under controlled test data.

### 3) Planner integration
- [ ] Logical/physical plan nodes for nearest (not executor-only special path)
- [ ] Cost model for exact vs ANN choice
- [ ] `EXPLAIN` includes vector plan info (mode, index, candidates, metric)

Acceptance tests:
- Planner deterministically picks exact for small sets, ANN for large indexed sets.
- Explain output matches actual execution mode.
- No regression in existing non-vector query planning.

### 4) Vector index management APIs
- [ ] Create/list/update/drop vector index endpoints
- [ ] Rebuild/resume endpoints
- [ ] Index health/state endpoint

Acceptance tests:
- API contract tests for all index lifecycle endpoints.
- Invalid configs return structured non-2xx errors.
- Explorer can create and inspect vector indexes end-to-end.

### 5) Filter + vector fusion correctness
- [ ] Define and enforce semantics for `where + nearest`
- [ ] Implement pre-filter/post-filter strategy selection
- [ ] Ensure deterministic ordering with ties

Acceptance tests:
- Filtered nearest result set equals reference implementation.
- Tie-breaking is stable across repeated runs.
- Mixed filter+nearest performance remains bounded at scale.

### 6) Reliability and performance SLO harness
- [ ] Benchmark suite for recall/latency/QPS
- [ ] Concurrency soak tests (writes + nearest reads)
- [ ] Memory/disk footprint tests and threshold guards

Acceptance tests:
- Meets p95/p99 latency and recall targets in CI/perf env.
- 24h soak test with no corruption/leak/degradation.
- Backpressure/limits triggered gracefully under load.

---

## P1: Production Maturity

### 1) Quantization + rerank
- [ ] Scalar quantization index mode
- [ ] Optional binary quantization mode
- [ ] Exact rerank on top candidates

Acceptance tests:
- Memory reduction target achieved.
- Recall drop stays within configured bound.

### 2) Operational observability
- [ ] Metrics: index build progress, ANN usage rate, candidate counts, errors
- [ ] Tracing from API -> planner -> executor -> storage
- [ ] Alerts for degraded/failed vector index states

Acceptance tests:
- Dashboards expose all required vector SLO signals.
- Simulated failures trigger expected alerts.

### 3) Security/multi-tenant controls
- [ ] Authz for vector index/query operations
- [ ] Quotas and rate limits per tenant
- [ ] Audit logs for vector operations

Acceptance tests:
- Permission matrix tests pass for vector endpoints.
- Tenant limits enforced without cross-tenant impact.

---

## P2: Advanced (Optional/Strategic)

- [ ] Hybrid retrieval (vector + full-text)
- [ ] Multi-vector fields / late interaction
- [ ] Distributed vector shard routing + cross-shard merge optimization
- [ ] Mojo acceleration for hot kernels where measurable

---

## Proposed Execution Order

1. Durable index metadata + WAL mutation records.
2. HNSW ANN storage path with exact fallback.
3. Planner integration and explain.
4. API/index lifecycle and Explorer wiring.
5. Filter fusion and deterministic ordering guarantees.
6. Reliability/performance SLO harness.
7. Quantization + rerank.
8. Security and observability completion.

---

## Ownership Boundaries (Owl Architecture)

- Zig (storage/control-plane core):
  - vector index persistence
  - ANN structures and search kernels
  - WAL/recovery coupling
- Rust (data plane/orchestration):
  - OQL syntax/validation
  - planning/execution strategy
  - API contracts + Explorer integration

---

## Definition of Done (Vector Production)

Vectors are production-grade only when all P0 items are complete and passing:
- durability/recovery correctness
- ANN latency+recall targets
- planner integration with explain
- operational APIs and observability
- reliability soak and benchmark gates
