# Next-Gen Owl with Deterministic Simulation Testing

## Specification Overview

This specification defines the complete implementation plan for transforming Owl from a single-node research prototype into a production-ready, distributed graph database with world-class testing infrastructure.

## Documents

### 1. **requirements.md** - What We're Building
**20 requirements** covering:
- O(1) graph navigation (preserved)
- Distributed horizontal scaling (new)
- Strong durability guarantees (new)
- Replication with consensus (new)
- Deterministic simulation testing (new)
- Performance targets
- Production readiness

**Read this first** to understand what success looks like.

### 2. **design.md** - How We're Building It
**Architecture and design** including:
- GlobalObjRef (128-bit distributed pointers)
- Consistent hashing for sharding
- WAL-first durability
- Raft consensus
- Simulation framework
- Dependency injection pattern

**Read this second** to understand the technical approach.

### 3. **tasks.md** - Implementation Plan
**80 tasks** organized into 5 phases:
- Phase 1: Simulation Foundation (Months 1-3)
- Phase 2: Distributed Infrastructure (Months 4-9)
- Phase 3: Performance Optimization (Months 10-12)
- Phase 4: Production Readiness (Months 13-14)
- Phase 5: OQL Compatibility (integrated throughout)

**Read this third** to understand the execution plan.

---

## Key Innovations

### 1. GlobalObjRef: Distributed O(1) Navigation
```zig
/// 128-bit global pointer extending current ObjRef
/// Layout: [Shard ID: 32][Local Offset: 64][Flags: 32]
pub const GlobalObjRef = extern struct {
    shard_id: u32,      // Which shard owns this object (0-4B shards)
    local_offset: u64,  // Offset within shard's arena (same as current ObjRef)
    flags: u32,         // Compression, type hints, version info
    
    /// Convert to local ObjRef for same-shard navigation
    pub inline fn toLocal(self: GlobalObjRef) ObjRef {
        return ObjRef.init(self.local_offset);
    }
    
    /// Check if reference is local to current shard
    pub inline fn isLocal(self: GlobalObjRef, current_shard: u32) bool {
        return self.shard_id == current_shard;
    }
    
    /// O(1) navigation: <50ns local, <1μs remote
    pub fn navigate(self: GlobalObjRef, arena: *Arena, network: *Network) !Object {
        if (self.isLocal(arena.shard_id)) {
            // Fast path: Direct pointer arithmetic (same as current Owl)
            return arena.read(self.toLocal());
        } else {
            // Remote path: Network call with caching
            return network.readRemote(self);
        }
    }
};
```

**Innovation**: Extends current ObjRef with shard location → transparent distributed navigation
- Local: <50ns (identical to current implementation)
- Remote: <1μs (network hop + cache)
- **Backward compatible**: Current ObjRef becomes GlobalObjRef with shard_id=0

### 2. Deterministic Simulation Testing
```zig
// Simulation test with OQL integration
test "raft_consensus_with_network_partition" {
    const seed = 42; // Reproducible!
    var sim = try Simulation.init(testing.allocator, seed);
    defer sim.deinit();
    
    // Create 5-node cluster
    const nodes = try sim.createCluster(5);
    
    // Insert test data using OQL
    try nodes[0].execute(
        \\User.insert {
        \\    id: "user1",
        \\    name: "Alice",
        \\    profile: {
        \\        email: "alice@test.com",
        \\        age: 28
        \\    }
        \\}
    );
    
    // Simulate network partition (isolate leader)
    sim.network.partition(&.{0}, &.{1, 2, 3, 4});
    
    // Advance time to trigger leader election
    sim.advanceTime(std.time.Duration.fromSecs(3));
    
    // Verify: Exactly one leader in majority partition
    const leaders = sim.countLeaders();
    try testing.expect(leaders == 1);
    
    // Verify: OQL queries work on new leader
    const result = try nodes[1].execute("User.where { .name == \"Alice\" }");
    try testing.expect(result.len == 1);
}
```

**Innovation**: Control all non-determinism → test millions of OQL scenarios
- Same seed = same result (100% reproducible)
- Runs 1000x faster than real-time
- Tests OQL queries under distributed failures
- Catches bugs before production

### 3. WAL-First Durability with OQL Integration
```zig
/// Atomic write with durability guarantee for OQL operations
pub fn executeOQLWrite(self: *Arena, oql: []const u8) !GlobalObjRef {
    // 1. Parse OQL statement
    const parsed = try OQLParser.parse(self.allocator, oql);
    defer parsed.deinit();
    
    // 2. Convert to object representation
    const obj = try Object.fromOQL(self.allocator, parsed);
    defer obj.deinit();
    
    // 3. Reserve space in arena (don't write yet)
    const objref = try self.reserve(obj.totalSize());
    
    // 4. Write to WAL with fsync (DURABLE)
    const lsn = try self.wal.appendWithSync(obj, objref);
    
    // 5. Write to arena (now safe - WAL has it)
    try self.commitReservation(objref, obj);
    
    // 6. Update indexes for OQL queries
    try self.updateIndexes(obj.id(), objref, lsn);
    
    return objref;
}

// Example OQL write that's guaranteed durable:
// User.insert { id: "u1", name: "Alice", age: 28 }
```

**Innovation**: WAL before arena → guaranteed durability for OQL operations
- If OQL write returns Ok, data survives crash/power loss
- Fast recovery (seconds, not minutes)
- **OQL-aware**: Indexes updated atomically with data

---

## Success Metrics

### Performance Targets
| Metric | Target | How We'll Achieve It |
|--------|--------|---------------------|
| Local navigation | <50ns | Direct pointer arithmetic (preserved) |
| Remote navigation | <1μs | Network + caching |
| Write throughput | 10M+ TPS | Batch operations + lock-free |
| Read throughput | 5M+ QPS | Co-location + caching |
| Replication lag | <10ms | Raft with pipelining |
| Failover time | <2s | Raft leader election |

### Quality Targets
- **100% OQL compatibility**: All existing queries work unchanged
- **1000+ simulation scenarios**: Comprehensive distributed testing
- **Zero data loss**: Strong durability guarantees
- **Zero split-brain**: Raft consensus prevents it
- **Production ready**: Monitoring, backups, documentation

---

## Timeline and Resources

### Duration: 14 Months

**Phase 1** (Months 1-3): Simulation Foundation
- **Goal**: Single-node simulation working
- **Team**: 2 engineers
- **Deliverable**: Can test concurrency bugs deterministically

**Phase 2** (Months 4-9): Distributed Infrastructure
- **Goal**: Multi-node cluster with Raft
- **Team**: 4 engineers
- **Deliverable**: Distributed simulation tests passing

**Phase 3** (Months 10-12): Performance Optimization
- **Goal**: Meet performance targets
- **Team**: 4 engineers
- **Deliverable**: 10M+ TPS, <50ns navigation

**Phase 4** (Months 13-14): Production Readiness
- **Goal**: Deploy to production
- **Team**: 6 engineers (+ ops)
- **Deliverable**: Production deployment

### Investment
- **Engineering**: 4-6 engineers × 14 months
- **Infrastructure**: Test clusters, CI/CD
- **Total**: ~$1-2M

### ROI
- **Prevented outages**: $1M+ (data loss, downtime)
- **Faster development**: 2-3x (instant feedback)
- **Customer confidence**: Priceless
- **Competitive advantage**: Unique in market

**ROI**: 20x+

---

## Risk Assessment

### Technical Risks

**Risk 1**: Distributed pointers too complex
- **Mitigation**: Start with single-shard, add distribution incrementally
- **Fallback**: Keep single-node mode as option

**Risk 2**: Simulation framework incomplete
- **Mitigation**: Build incrementally, test at each step
- **Fallback**: Use traditional integration tests

**Risk 3**: Performance regressions
- **Mitigation**: Continuous benchmarking, performance tests
- **Fallback**: Optimize hot paths, profile-guided optimization

### Schedule Risks

**Risk 1**: Underestimated complexity
- **Mitigation**: 20% buffer in timeline
- **Fallback**: Cut optional features (columnar storage, etc.)

**Risk 2**: Team availability
- **Mitigation**: Cross-train team members
- **Fallback**: Extend timeline or reduce scope

### Market Risks

**Risk 1**: Competitors ship first
- **Mitigation**: Focus on unique advantages (O(1) navigation + DST)
- **Fallback**: Emphasize quality over speed

---

## Dependencies

### External Dependencies
- **Rust**: 1.70+ (stable)
- **Zig**: 0.12+ (for storage engine)
- **Libraries**: tokio, serde, proptest, etc.

### Internal Dependencies
- **Current Owl**: Must maintain compatibility
- **OQL Test Suite**: Must pass 100%
- **Zig Storage**: Already implemented (Phase 4 complete)

### Infrastructure Dependencies
- **CI/CD**: GitHub Actions or similar
- **Test Clusters**: For integration testing
- **Monitoring**: Prometheus + Grafana

---

## Decision Points

### Month 3: Continue or Pivot?
**Decision**: Does single-node simulation work?
- **If YES**: Proceed to Phase 2 (distributed)
- **If NO**: Reassess simulation approach

### Month 9: Ship or Optimize?
**Decision**: Are distributed tests passing?
- **If YES**: Proceed to Phase 3 (optimization)
- **If NO**: Extend Phase 2, cut optional features

### Month 12: Production Ready?
**Decision**: Do we meet performance targets?
- **If YES**: Proceed to Phase 4 (production)
- **If NO**: Extend optimization, adjust targets

---

## Success Criteria

### Phase 1 Success
- [ ] SimulatedClock working
- [ ] SimulatedScheduler working
- [ ] SimulatedFileSystem working
- [ ] 10+ single-node simulation tests passing
- [ ] Found at least 1 real bug via simulation

### Phase 2 Success
- [ ] GlobalObjRef implemented
- [ ] Raft consensus working
- [ ] SimulatedNetwork working
- [ ] 20+ distributed simulation tests passing
- [ ] 3-node cluster with automatic failover

### Phase 3 Success
- [ ] 10M+ writes/sec achieved
- [ ] <50ns local navigation maintained
- [ ] Lock-free structures implemented
- [ ] Compression working (5-10x reduction)

### Phase 4 Success
- [ ] Monitoring/observability complete
- [ ] Backup/restore working
- [ ] Documentation complete
- [ ] Production deployment successful
- [ ] 100% OQL test suite passing

---

## Next Steps

1. **Review** this spec with team
2. **Approve** requirements and design
3. **Assign** engineers to Phase 1
4. **Start** with Epic 1 (Core Simulation Infrastructure)
5. **Iterate** based on learnings

---

## References

### Related Documents
- **OWL_WEAKNESSES_ANALYSIS.md**: Analysis of current limitations
- **OWL_NEXT_GEN_DESIGN.md**: Detailed next-gen architecture
- **DETERMINISTIC_SIMULATION_PART1-3.md**: DST deep dive
- **DST_REASONING.md**: Why DST is critical

### External References
- **FoundationDB**: "Testing Distributed Systems" (Will Wilson)
- **TigerBeetle**: Simulation testing from day one
- **Raft Paper**: "In Search of an Understandable Consensus Algorithm"
- **Jepsen**: Kyle Kingsbury's distributed systems testing

---

## Conclusion

This specification provides a complete roadmap for transforming Owl into a production-ready, distributed graph database with world-class testing infrastructure. The combination of O(1) navigation, strong durability, and deterministic simulation testing creates a unique competitive advantage in the market.

**Let's build it right.**
