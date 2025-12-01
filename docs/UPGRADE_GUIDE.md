# Owl Database Upgrade Guide

## Overview

This guide provides instructions for upgrading Owl Database to newer versions, including rolling upgrade procedures, compatibility information, and migration tools.

## Table of Contents

1. [Version Compatibility Matrix](#version-compatibility-matrix)
2. [Pre-Upgrade Checklist](#pre-upgrade-checklist)
3. [Rolling Upgrade Procedure](#rolling-upgrade-procedure)
4. [Migration Tools](#migration-tools)
5. [Rollback Procedures](#rollback-procedures)
6. [Troubleshooting](#troubleshooting)

---

## Version Compatibility Matrix

### Supported Upgrade Paths

| From Version | To Version | Direct Upgrade | Notes |
|--------------|------------|----------------|-------|
| 0.1.x | 0.2.x | ✓ Yes | OQL syntax unchanged |
| 0.1.x | 1.0.x | ✓ Yes | Requires data migration |
| 0.2.x | 1.0.x | ✓ Yes | Minor schema changes |
| 1.0.x | 1.1.x | ✓ Yes | Rolling upgrade supported |
| 1.0.x | 2.0.x | ⚠ Requires migration | Breaking changes in storage format |

### Protocol Compatibility

| Version | Raft Protocol | Storage Format | OQL Version |
|---------|---------------|----------------|-------------|
| 0.1.x | N/A (single-node) | v1 | 1.0 |
| 0.2.x | v1 | v1 | 1.0 |
| 1.0.x | v2 | v2 | 1.1 |
| 1.1.x | v2 | v2 | 1.1 |
| 2.0.x | v3 | v3 | 2.0 |

### Feature Compatibility

| Feature | Introduced | Deprecated | Removed |
|---------|-----------|------------|---------|
| Single-node mode | 0.1.0 | - | - |
| Distributed mode | 0.2.0 | - | - |
| GlobalObjRef | 1.0.0 | - | - |
| Lock-free concurrency | 1.0.0 | - | - |
| Adaptive compression | 1.0.0 | - | - |
| Columnar storage | 1.1.0 | - | - |
| RocksDB backend | 0.1.0 | 1.0.0 | 1.1.0 |

---

## Pre-Upgrade Checklist

Before starting an upgrade, complete the following checklist:

### 1. Backup Your Data

```bash
# Create a full backup
owl-admin backup create --destination /backups/pre-upgrade --type full

# Verify backup integrity
owl-admin backup verify /backups/pre-upgrade
```

### 2. Check Cluster Health

```bash
# Verify all nodes are healthy
owl-admin health check

# Check replication status
owl-admin health replication
```

Expected output:
```
Overall Status: Healthy
All nodes: Healthy
Replication lag: <10ms
```

### 3. Review Release Notes

- Read the release notes for the target version
- Identify breaking changes that affect your application
- Review deprecated features you may be using

### 4. Test in Staging

- Deploy the new version to a staging environment
- Run your application test suite
- Verify OQL queries work as expected
- Check performance benchmarks

### 5. Plan Maintenance Window

For major version upgrades (e.g., 1.x → 2.x):
- Schedule a maintenance window
- Notify users of potential downtime
- Prepare rollback plan

For minor version upgrades (e.g., 1.0 → 1.1):
- Rolling upgrade with zero downtime
- No maintenance window required

---

## Rolling Upgrade Procedure

Rolling upgrades allow you to upgrade a cluster without downtime. This is supported for minor version upgrades within the same major version.

### Prerequisites

- Cluster must be healthy (all nodes reachable)
- Replication factor ≥ 3 (for fault tolerance)
- All nodes running the same version before upgrade

### Step-by-Step Procedure

#### Step 1: Upgrade Follower Nodes

Upgrade follower nodes one at a time:

```bash
# On each follower node:

# 1. Stop the Owl service
sudo systemctl stop owl

# 2. Backup the current binary
sudo cp /usr/local/bin/owl /usr/local/bin/owl.backup

# 3. Install new version
sudo wget https://releases.owl-db.io/v1.1.0/owl-linux-amd64
sudo mv owl-linux-amd64 /usr/local/bin/owl
sudo chmod +x /usr/local/bin/owl

# 4. Start the service
sudo systemctl start owl

# 5. Verify node rejoined cluster
owl-admin cluster nodes
```

**Wait for node to catch up before proceeding to next node:**

```bash
# Check replication lag
owl-admin health replication

# Expected: Lag <100ms for upgraded node
```

Repeat for each follower node.

#### Step 2: Upgrade Leader Node

Once all followers are upgraded:

```bash
# 1. Trigger leader election to move leadership
owl-admin cluster transfer-leadership --to-node <follower-node-id>

# 2. Wait for new leader election (typically <2 seconds)
owl-admin cluster status

# 3. Upgrade the old leader (now a follower)
# Follow same steps as follower upgrade above
```

#### Step 3: Verify Cluster

```bash
# Check all nodes are running new version
owl-admin cluster nodes

# Verify cluster health
owl-admin health check

# Run smoke tests
owl-admin test smoke
```

### Rolling Upgrade Timeline

For a 3-node cluster:
- **Total time**: ~15-20 minutes
- **Downtime**: 0 seconds (writes continue throughout)
- **Per-node upgrade**: ~5 minutes

For a 5-node cluster:
- **Total time**: ~25-30 minutes
- **Downtime**: 0 seconds

---

## Migration Tools

### Data Migration Tool

For major version upgrades that require data migration:

```bash
# Run migration tool
owl-migrate --from-version 1.0 --to-version 2.0 \
  --data-dir /var/lib/owl/data \
  --output-dir /var/lib/owl/data-migrated

# Verify migration
owl-migrate verify --data-dir /var/lib/owl/data-migrated
```

### Schema Migration

If schema changes are required:

```bash
# Export current schema
owl-admin schema export --output schema-v1.json

# Apply schema migration
owl-admin schema migrate --from schema-v1.json --to schema-v2.json

# Verify schema compatibility
owl-admin schema validate
```

### OQL Query Migration

For OQL syntax changes:

```bash
# Analyze OQL queries for compatibility
owl-migrate analyze-queries --input queries.oql

# Auto-migrate queries (where possible)
owl-migrate update-queries --input queries.oql --output queries-v2.oql

# Review changes
diff queries.oql queries-v2.oql
```

---

## Rollback Procedures

### Rolling Back a Single Node

If a node fails after upgrade:

```bash
# 1. Stop the service
sudo systemctl stop owl

# 2. Restore previous binary
sudo cp /usr/local/bin/owl.backup /usr/local/bin/owl

# 3. Restart service
sudo systemctl start owl

# 4. Verify node rejoined
owl-admin cluster nodes
```

### Rolling Back Entire Cluster

If the upgrade fails cluster-wide:

#### Option 1: Binary Rollback (Fast)

```bash
# On each node:
sudo systemctl stop owl
sudo cp /usr/local/bin/owl.backup /usr/local/bin/owl
sudo systemctl start owl
```

#### Option 2: Restore from Backup (Slower, but guaranteed)

```bash
# 1. Stop all nodes
owl-admin cluster stop-all

# 2. Restore from backup
owl-admin backup restore --source /backups/pre-upgrade

# 3. Start cluster
owl-admin cluster start-all

# 4. Verify cluster health
owl-admin health check
```

### Rollback Considerations

- **Data loss**: Rolling back may lose data written after upgrade started
- **Compatibility**: Ensure backup is compatible with rollback version
- **Testing**: Test rollback procedure in staging before production

---

## Troubleshooting

### Common Issues

#### Issue: Node fails to rejoin cluster after upgrade

**Symptoms:**
```
Node status: Unreachable
Error: Connection refused
```

**Solution:**
```bash
# Check logs
sudo journalctl -u owl -n 100

# Verify network connectivity
owl-admin cluster ping <node-id>

# Check configuration
owl-admin config validate

# Restart node
sudo systemctl restart owl
```

#### Issue: High replication lag after upgrade

**Symptoms:**
```
Replication lag: 5000ms (Critical)
```

**Solution:**
```bash
# Check node resources
owl-admin health node <node-id>

# Verify network bandwidth
owl-admin cluster network-test

# Increase replication timeout (if needed)
owl-admin config set replication.timeout_ms 10000
```

#### Issue: OQL queries fail after upgrade

**Symptoms:**
```
Error: Unknown function 'new_function'
Error: Syntax error near 'new_keyword'
```

**Solution:**
```bash
# Check OQL version compatibility
owl-admin version --oql

# Review breaking changes
owl-admin changelog --version <target-version>

# Update queries
owl-migrate analyze-queries --input queries.oql
```

#### Issue: Performance degradation after upgrade

**Symptoms:**
```
P99 latency: 50ms (was 1ms before upgrade)
Throughput: 1M QPS (was 5M QPS before upgrade)
```

**Solution:**
```bash
# Run performance diagnostics
owl-admin perf diagnose

# Check for configuration changes
owl-admin config diff --before /backups/pre-upgrade/config.toml

# Review new features that may need tuning
owl-admin config show performance

# Rollback if critical
owl-admin rollback --to-backup /backups/pre-upgrade
```

### Getting Help

If you encounter issues during upgrade:

1. **Check logs**: `sudo journalctl -u owl -n 1000`
2. **Review documentation**: https://docs.owl-db.io/upgrade
3. **Community forum**: https://community.owl-db.io
4. **GitHub issues**: https://github.com/owl-db/owl/issues
5. **Enterprise support**: support@owl-db.io (for enterprise customers)

---

## Version-Specific Upgrade Notes

### Upgrading to 1.0.0

**Major Changes:**
- Introduction of distributed mode with GlobalObjRef
- Raft consensus for replication
- Lock-free concurrency primitives
- Adaptive compression

**Breaking Changes:**
- RocksDB backend deprecated (use Zig storage engine)
- Storage format changed (requires migration)

**Migration Steps:**
```bash
# 1. Backup data
owl-admin backup create --destination /backups/v0.2-to-v1.0 --type full

# 2. Stop cluster
owl-admin cluster stop-all

# 3. Run migration tool
owl-migrate --from-version 0.2 --to-version 1.0 \
  --data-dir /var/lib/owl/data

# 4. Update configuration
owl-admin config migrate --from v0.2 --to v1.0

# 5. Start cluster with new version
owl-admin cluster start-all

# 6. Verify migration
owl-admin health check
owl-admin test smoke
```

### Upgrading to 1.1.0

**Major Changes:**
- Columnar storage for analytical workloads
- Batch operations optimization
- Enhanced observability

**Breaking Changes:**
- None (backward compatible)

**Migration Steps:**
```bash
# Rolling upgrade (zero downtime)
# Follow standard rolling upgrade procedure above
```

### Upgrading to 2.0.0

**Major Changes:**
- OQL 2.0 with new syntax features
- Enhanced distributed query optimization
- New storage format v3

**Breaking Changes:**
- OQL syntax changes (some queries need updates)
- Storage format incompatible with 1.x
- Minimum Raft protocol version: v3

**Migration Steps:**
```bash
# 1. Backup and test in staging
owl-admin backup create --destination /backups/v1-to-v2 --type full

# 2. Analyze OQL queries
owl-migrate analyze-queries --input queries.oql

# 3. Update queries (manual review required)
owl-migrate update-queries --input queries.oql --output queries-v2.oql

# 4. Schedule maintenance window
# 5. Stop cluster
owl-admin cluster stop-all

# 6. Run migration
owl-migrate --from-version 1.x --to-version 2.0 \
  --data-dir /var/lib/owl/data

# 7. Start cluster with v2.0
owl-admin cluster start-all

# 8. Verify and test
owl-admin health check
owl-admin test comprehensive
```

---

## Best Practices

### 1. Always Backup Before Upgrade

```bash
# Full backup before any upgrade
owl-admin backup create --destination /backups/$(date +%Y%m%d) --type full
```

### 2. Test in Staging First

- Deploy to staging environment
- Run full test suite
- Verify performance benchmarks
- Test rollback procedure

### 3. Monitor During Upgrade

```bash
# Monitor cluster health in real-time
owl-admin health monitor --interval 5
```

### 4. Upgrade During Low Traffic

- Schedule upgrades during off-peak hours
- Monitor traffic patterns before upgrade
- Have rollback plan ready

### 5. Document Your Upgrade

- Record upgrade start/end times
- Note any issues encountered
- Document configuration changes
- Update runbooks

---

## Automated Upgrade Scripts

### Rolling Upgrade Script

```bash
#!/bin/bash
# rolling-upgrade.sh - Automated rolling upgrade script

set -e

NEW_VERSION=$1
BACKUP_DIR="/backups/upgrade-$(date +%Y%m%d-%H%M%S)"

echo "Starting rolling upgrade to version $NEW_VERSION"

# 1. Pre-upgrade checks
echo "Step 1: Pre-upgrade checks..."
owl-admin health check || exit 1
owl-admin backup create --destination $BACKUP_DIR --type full

# 2. Get cluster nodes
NODES=$(owl-admin cluster nodes --format json | jq -r '.[] | select(.role != "Leader") | .id')
LEADER=$(owl-admin cluster nodes --format json | jq -r '.[] | select(.role == "Leader") | .id')

# 3. Upgrade followers
for NODE in $NODES; do
    echo "Upgrading follower node $NODE..."
    ssh node-$NODE "sudo systemctl stop owl && \
                     sudo cp /usr/local/bin/owl /usr/local/bin/owl.backup && \
                     sudo wget -O /usr/local/bin/owl https://releases.owl-db.io/$NEW_VERSION/owl-linux-amd64 && \
                     sudo chmod +x /usr/local/bin/owl && \
                     sudo systemctl start owl"
    
    # Wait for node to catch up
    sleep 10
    owl-admin health node $NODE || exit 1
done

# 4. Transfer leadership
echo "Transferring leadership from node $LEADER..."
FIRST_FOLLOWER=$(echo $NODES | awk '{print $1}')
owl-admin cluster transfer-leadership --to-node $FIRST_FOLLOWER

# 5. Upgrade old leader
echo "Upgrading old leader node $LEADER..."
ssh node-$LEADER "sudo systemctl stop owl && \
                   sudo cp /usr/local/bin/owl /usr/local/bin/owl.backup && \
                   sudo wget -O /usr/local/bin/owl https://releases.owl-db.io/$NEW_VERSION/owl-linux-amd64 && \
                   sudo chmod +x /usr/local/bin/owl && \
                   sudo systemctl start owl"

# 6. Verify upgrade
echo "Verifying upgrade..."
sleep 10
owl-admin health check || exit 1
owl-admin cluster nodes

echo "Rolling upgrade complete!"
```

---

## Compatibility Testing

### Test Your Application

Before upgrading production:

```bash
# 1. Deploy new version to test environment
# 2. Run application test suite
npm test  # or your test command

# 3. Run OQL compatibility tests
owl-admin test oql-compatibility --queries queries.oql

# 4. Run performance benchmarks
owl-admin benchmark --duration 60s

# 5. Test failure scenarios
owl-admin test failure-injection
```

### Verify OQL Compatibility

```bash
# Test all OQL queries
owl-migrate test-queries --input queries.oql --version $NEW_VERSION

# Expected output:
# ✓ 245/245 queries compatible
# ⚠ 0 queries need updates
# ✗ 0 queries incompatible
```

---

## Support and Resources

- **Documentation**: https://docs.owl-db.io
- **Release Notes**: https://github.com/owl-db/owl/releases
- **Community Forum**: https://community.owl-db.io
- **Slack Channel**: https://owl-db.slack.com
- **Enterprise Support**: support@owl-db.io

---

## Appendix: Upgrade Checklist

Print this checklist and check off items during upgrade:

```
Pre-Upgrade:
[ ] Read release notes
[ ] Create full backup
[ ] Verify backup integrity
[ ] Test in staging environment
[ ] Review breaking changes
[ ] Plan maintenance window (if needed)
[ ] Notify stakeholders

During Upgrade:
[ ] Verify cluster health
[ ] Upgrade follower nodes (one at a time)
[ ] Verify each node after upgrade
[ ] Transfer leadership
[ ] Upgrade leader node
[ ] Verify cluster health

Post-Upgrade:
[ ] Run smoke tests
[ ] Monitor performance
[ ] Check application logs
[ ] Verify OQL queries
[ ] Update documentation
[ ] Notify stakeholders of completion

Rollback (if needed):
[ ] Stop affected nodes
[ ] Restore previous binary
[ ] Restart nodes
[ ] Verify cluster health
[ ] Document issues encountered
```

---

**Last Updated**: November 26, 2025  
**Document Version**: 1.0  
**Owl Database Version**: 1.0.0+
