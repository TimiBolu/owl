# Owl Database Operations Manual

## Overview

This manual provides operational procedures and best practices for running Owl Database in production.

## Table of Contents

1. [Daily Operations](#daily-operations)
2. [Monitoring and Alerting](#monitoring-and-alerting)
3. [Backup and Recovery](#backup-and-recovery)
4. [Performance Tuning](#performance-tuning)
5. [Troubleshooting](#troubleshooting)
6. [Maintenance Procedures](#maintenance-procedures)
7. [Incident Response](#incident-response)
8. [Capacity Planning](#capacity-planning)

---

## Daily Operations

### Health Checks

Run daily health checks to ensure cluster stability:

```bash
# Morning health check
owl-admin health check

# Check replication status
owl-admin health replication

# Review cluster status
owl-admin cluster status
```

### Log Review

Check logs for errors or warnings:

```bash
# View recent logs
sudo journalctl -u owl -n 100

# Check for errors
sudo journalctl -u owl | grep -i error

# Check for warnings
sudo journalctl -u owl | grep -i warn
```

### Metrics Review

Review key metrics in Grafana dashboard:

- **Query Latency**: P50, P95, P99 should be <1ms
- **Throughput**: Writes (TPS), Reads (QPS)
- **Replication Lag**: Should be <10ms
- **Resource Usage**: CPU, Memory, Disk
- **Error Rate**: Should be <0.1%

---

## Monitoring and Alerting

### Prometheus Metrics

Owl exposes metrics at `http://localhost:9090/metrics`:

#### Key Metrics to Monitor

**Performance Metrics:**
- `owl_query_duration_seconds` - Query latency histogram
- `owl_queries_total` - Total queries executed
- `owl_writes_total` - Total writes executed
- `owl_errors_total` - Total errors

**Resource Metrics:**
- `owl_memory_usage_bytes` - Memory usage
- `owl_disk_usage_bytes` - Disk usage
- `owl_cpu_usage_percent` - CPU usage
- `owl_connections_active` - Active connections

**Replication Metrics:**
- `owl_replication_lag_ms` - Replication lag
- `owl_raft_leader` - Current leader (0 or 1)
- `owl_raft_term` - Current Raft term

### Recommended Alerts

#### Critical Alerts

```yaml
# Node Down
- alert: OwlNodeDown
  expr: up{job="owl"} == 0
  for: 1m
  severity: critical
  
# High Replication Lag
- alert: OwlHighReplicationLag
  expr: owl_replication_lag_ms > 1000
  for: 5m
  severity: critical

# Disk Space Low
- alert: OwlDiskSpaceLow
  expr: owl_disk_usage_percent > 90
  for: 5m
  severity: critical
```

#### Warning Alerts

```yaml
# High Query Latency
- alert: OwlHighLatency
  expr: owl_query_duration_seconds{quantile="0.99"} > 0.01
  for: 10m
  severity: warning

# High Error Rate
- alert: OwlHighErrorRate
  expr: rate(owl_errors_total[5m]) > 10
  for: 5m
  severity: warning

# Memory Usage High
- alert: OwlMemoryHigh
  expr: owl_memory_usage_percent > 85
  for: 10m
  severity: warning
```

### Grafana Dashboards

Import the official Owl dashboard:

```bash
# Download dashboard
wget https://grafana.com/api/dashboards/owl-db/revisions/latest/download

# Import to Grafana
# Navigate to Grafana UI → Dashboards → Import
```

---

## Backup and Recovery

### Backup Strategy

#### Full Backups

Run full backups daily during off-peak hours:

```bash
# Create full backup
owl-admin backup create \
  --destination /backups/$(date +%Y%m%d) \
  --type full

# Verify backup
owl-admin backup verify /backups/$(date +%Y%m%d)
```

#### Incremental Backups

Run incremental backups every 6 hours:

```bash
# Create incremental backup
owl-admin backup create \
  --destination /backups/$(date +%Y%m%d-%H%M) \
  --type incremental
```

#### Automated Backup Script

```bash
#!/bin/bash
# /usr/local/bin/owl-backup.sh

BACKUP_DIR="/backups"
DATE=$(date +%Y%m%d)
TIME=$(date +%H%M)

# Full backup at midnight
if [ "$TIME" = "0000" ]; then
    owl-admin backup create \
      --destination "$BACKUP_DIR/full-$DATE" \
      --type full
else
    # Incremental backup every 6 hours
    owl-admin backup create \
      --destination "$BACKUP_DIR/incr-$DATE-$TIME" \
      --type incremental
fi

# Verify backup
owl-admin backup verify "$BACKUP_DIR/full-$DATE" || \
owl-admin backup verify "$BACKUP_DIR/incr-$DATE-$TIME"

# Clean up old backups (keep 30 days)
find "$BACKUP_DIR" -type d -mtime +30 -exec rm -rf {} \;
```

Add to crontab:

```bash
# Run every 6 hours
0 */6 * * * /usr/local/bin/owl-backup.sh
```

### Recovery Procedures

#### Restore from Full Backup

```bash
# 1. Stop cluster
owl-admin cluster stop-all

# 2. Restore from backup
owl-admin backup restore \
  --source /backups/20251126

# 3. Start cluster
owl-admin cluster start-all

# 4. Verify
owl-admin health check
```

#### Point-in-Time Recovery

```bash
# Restore to specific LSN
owl-admin backup restore \
  --source /backups/20251126 \
  --lsn 123456

# Verify recovery
owl-admin health check
```

---

## Performance Tuning

### Query Optimization

#### Identify Slow Queries

```bash
# Check slow query log
sudo journalctl -u owl | grep "slow query"

# Get query statistics
owl-admin stats queries --sort-by duration
```

#### Optimize Queries

1. **Add Indexes**: Create indexes on frequently queried fields
2. **Use Projections**: Select only needed fields
3. **Batch Operations**: Use batch inserts/updates
4. **Cache Results**: Cache frequently accessed data

### Resource Optimization

#### Memory Tuning

```bash
# Increase cache size
owl-admin config set performance.cache_size_mb 16384

# Adjust arena size
owl-admin config set storage.max_arena_size_gb 500
```

#### CPU Tuning

```bash
# Increase worker threads
owl-admin config set performance.worker_threads 32

# Enable SIMD optimizations
owl-admin config set advanced.simd_enabled true
```

#### Disk Tuning

```bash
# Use separate disks for data and WAL
owl-admin config set storage.data_dir /nvme0/owl/data
owl-admin config set storage.wal_dir /nvme1/owl/wal

# Enable compression
owl-admin config set storage.compression_enabled true
```

### Network Optimization

```bash
# Increase connection limit
owl-admin config set performance.max_connections 10000

# Tune TCP settings (Linux)
sudo sysctl -w net.core.somaxconn=4096
sudo sysctl -w net.ipv4.tcp_max_syn_backlog=4096
```

---

## Troubleshooting

### Common Issues

#### High Query Latency

**Symptoms:**
- P99 latency >10ms
- Slow application response

**Diagnosis:**
```bash
# Check resource usage
owl-admin health node <node-id>

# Check slow queries
sudo journalctl -u owl | grep "slow query"

# Check cache hit rate
owl-admin stats cache
```

**Solutions:**
1. Increase cache size
2. Add indexes
3. Optimize queries
4. Scale horizontally

#### High Replication Lag

**Symptoms:**
- Replication lag >100ms
- Followers falling behind

**Diagnosis:**
```bash
# Check replication status
owl-admin health replication

# Check network latency
owl-admin cluster network-test

# Check follower resources
owl-admin health node <follower-id>
```

**Solutions:**
1. Check network connectivity
2. Increase follower resources
3. Reduce write load
4. Tune replication settings

#### Node Crashes

**Symptoms:**
- Node unreachable
- Service not running

**Diagnosis:**
```bash
# Check service status
sudo systemctl status owl

# Check logs
sudo journalctl -u owl -n 500

# Check system resources
top
df -h
```

**Solutions:**
1. Check for OOM kills: `dmesg | grep -i oom`
2. Check disk space: `df -h`
3. Review crash logs
4. Restart service: `sudo systemctl restart owl`

#### Split-Brain Scenario

**Symptoms:**
- Multiple leaders
- Inconsistent data

**Diagnosis:**
```bash
# Check cluster status
owl-admin cluster status

# Check Raft state
owl-admin cluster raft-status
```

**Solutions:**
1. This should not happen with proper Raft implementation
2. If it does, stop all nodes
3. Restore from backup
4. Contact support

---

## Maintenance Procedures

### Adding a Node

```bash
# 1. Install Owl on new node
# (Follow installation guide)

# 2. Configure new node
# Edit /etc/owl/owl.toml with unique node_id

# 3. Add node to cluster
owl-admin cluster add-node \
  --address new-node.example.com:8080 \
  --id 4

# 4. Wait for rebalancing
owl-admin cluster rebalance

# 5. Verify
owl-admin cluster nodes
```

### Removing a Node

```bash
# 1. Drain node (move data to other nodes)
owl-admin cluster drain-node --node-id 3

# 2. Remove from cluster
owl-admin cluster remove-node 3

# 3. Stop service on removed node
ssh node3 "sudo systemctl stop owl"

# 4. Verify
owl-admin cluster nodes
```

### Upgrading Nodes

See [UPGRADE_GUIDE.md](UPGRADE_GUIDE.md) for detailed procedures.

### Rebalancing Data

```bash
# Check current distribution
owl-admin cluster shards

# Trigger rebalancing
owl-admin cluster rebalance

# Monitor progress
watch -n 5 'owl-admin cluster shards'
```

---

## Incident Response

### Incident Response Checklist

1. **Assess Severity**
   - Critical: Service down, data loss
   - High: Performance degradation
   - Medium: Single node issue
   - Low: Warning alerts

2. **Gather Information**
   ```bash
   owl-admin health check
   owl-admin cluster status
   sudo journalctl -u owl -n 500
   ```

3. **Communicate**
   - Notify stakeholders
   - Update status page
   - Document actions

4. **Mitigate**
   - Follow runbooks
   - Escalate if needed
   - Implement workarounds

5. **Resolve**
   - Fix root cause
   - Verify resolution
   - Monitor stability

6. **Post-Mortem**
   - Document incident
   - Identify improvements
   - Update runbooks

### Emergency Procedures

#### Complete Cluster Failure

```bash
# 1. Stop all nodes
owl-admin cluster stop-all

# 2. Restore from latest backup
owl-admin backup restore --source /backups/latest

# 3. Start cluster
owl-admin cluster start-all

# 4. Verify data integrity
owl-admin test comprehensive
```

#### Data Corruption

```bash
# 1. Identify corrupted node
owl-admin health check

# 2. Stop corrupted node
ssh node-X "sudo systemctl stop owl"

# 3. Restore node from backup
owl-admin backup restore \
  --source /backups/latest \
  --node node-X

# 4. Restart node
ssh node-X "sudo systemctl start owl"

# 5. Verify
owl-admin health node X
```

---

## Capacity Planning

### Growth Projections

Monitor these metrics for capacity planning:

- **Data Growth Rate**: GB/day
- **Query Growth Rate**: QPS increase
- **Connection Growth**: Active connections trend

### Scaling Triggers

#### Vertical Scaling (Add Resources)

Scale up when:
- CPU usage consistently >70%
- Memory usage consistently >80%
- Disk usage >85%

#### Horizontal Scaling (Add Nodes)

Scale out when:
- Query latency increasing despite optimization
- Single node at capacity
- Need higher availability

### Scaling Procedures

#### Add More Nodes

```bash
# Current: 3 nodes
# Target: 5 nodes

# Add node 4
owl-admin cluster add-node --address node4:8080 --id 4

# Add node 5
owl-admin cluster add-node --address node5:8080 --id 5

# Rebalance
owl-admin cluster rebalance
```

#### Increase Node Resources

```bash
# 1. Drain node
owl-admin cluster drain-node --node-id 1

# 2. Stop node
ssh node1 "sudo systemctl stop owl"

# 3. Upgrade hardware (CPU, RAM, Disk)

# 4. Update configuration
owl-admin config set performance.cache_size_mb 32768
owl-admin config set performance.worker_threads 64

# 5. Start node
ssh node1 "sudo systemctl start owl"

# 6. Verify
owl-admin health node 1
```

---

## Best Practices

### Operational Excellence

1. **Automate Everything**
   - Backups
   - Monitoring
   - Alerting
   - Recovery

2. **Document Procedures**
   - Runbooks for common issues
   - Incident response plans
   - Configuration changes

3. **Test Regularly**
   - Backup restoration
   - Failover procedures
   - Disaster recovery

4. **Monitor Proactively**
   - Set up alerts
   - Review metrics daily
   - Trend analysis

5. **Plan for Growth**
   - Capacity planning
   - Performance testing
   - Scaling strategy

### Security Best Practices

1. **Enable Authentication**
   ```bash
   owl-admin config set security.auth_enabled true
   ```

2. **Enable TLS**
   ```bash
   owl-admin config set security.tls_enabled true
   ```

3. **Rotate Credentials**
   - Change admin tokens monthly
   - Rotate TLS certificates annually

4. **Audit Logs**
   - Review access logs
   - Monitor for suspicious activity

5. **Network Security**
   - Use firewalls
   - Restrict access to cluster ports
   - Use VPN for remote access

---

## Support and Resources

- **Documentation**: https://docs.owl-db.io
- **Community Forum**: https://community.owl-db.io
- **GitHub Issues**: https://github.com/owl-db/owl/issues
- **Enterprise Support**: support@owl-db.io
- **Emergency Hotline**: +1-555-OWL-HELP (enterprise customers)

---

**Last Updated**: November 26, 2025  
**Document Version**: 1.0  
**Owl Database Version**: 1.0.0+
