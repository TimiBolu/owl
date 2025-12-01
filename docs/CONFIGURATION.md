# Owl Database Configuration Reference

## Overview

This document provides a comprehensive reference for all Owl Database configuration options.

## Table of Contents

1. [Configuration File Format](#configuration-file-format)
2. [Cluster Configuration](#cluster-configuration)
3. [Storage Configuration](#storage-configuration)
4. [Replication Configuration](#replication-configuration)
5. [Performance Configuration](#performance-configuration)
6. [Security Configuration](#security-configuration)
7. [Observability Configuration](#observability-configuration)
8. [Advanced Configuration](#advanced-configuration)
9. [Environment Variables](#environment-variables)
10. [Configuration Examples](#configuration-examples)

---

## Configuration File Format

Owl uses TOML format for configuration files. The default location is `/etc/owl/owl.toml`.

### Basic Structure

```toml
[cluster]
# Cluster-related settings

[storage]
# Storage engine settings

[replication]
# Replication and consensus settings

[performance]
# Performance tuning settings

[security]
# Security and authentication settings

[observability]
# Monitoring and logging settings
```

---

## Cluster Configuration

### `[cluster]` Section

#### `node_id`
- **Type**: Integer (u32)
- **Required**: Yes
- **Default**: None
- **Range**: 1 to 4,294,967,295
- **Description**: Unique identifier for this node in the cluster
- **Example**: `node_id = 1`

#### `listen_address`
- **Type**: String
- **Required**: Yes
- **Default**: `"0.0.0.0:8080"`
- **Format**: `"host:port"`
- **Description**: Address and port for the node to listen on
- **Example**: `listen_address = "0.0.0.0:8080"`
- **Notes**: 
  - Use `0.0.0.0` to listen on all interfaces
  - Use specific IP for single interface
  - Ensure port is not in use

#### `peers`
- **Type**: Array of Strings
- **Required**: No (Yes for distributed mode)
- **Default**: `[]`
- **Format**: `["host:port", ...]`
- **Description**: List of peer nodes in the cluster
- **Example**: `peers = ["node2.example.com:8080", "node3.example.com:8080"]`
- **Notes**:
  - Empty array for single-node mode
  - Must include all other nodes (not self)
  - Use hostnames or IP addresses

#### `advertise_address`
- **Type**: String
- **Required**: No
- **Default**: Same as `listen_address`
- **Format**: `"host:port"`
- **Description**: Address advertised to other nodes (useful for NAT)
- **Example**: `advertise_address = "public-ip:8080"`

---

## Storage Configuration

### `[storage]` Section

#### `data_dir`
- **Type**: String (Path)
- **Required**: Yes
- **Default**: `"/var/lib/owl/data"`
- **Description**: Directory for storing data files
- **Example**: `data_dir = "/var/lib/owl/data"`
- **Notes**:
  - Must have write permissions
  - Should be on fast SSD (NVMe recommended)
  - Ensure sufficient disk space

#### `wal_dir`
- **Type**: String (Path)
- **Required**: Yes
- **Default**: `"/var/lib/owl/wal"`
- **Description**: Directory for Write-Ahead Log files
- **Example**: `wal_dir = "/var/lib/owl/wal"`
- **Notes**:
  - Can be same as `data_dir` or separate
  - Separate disk recommended for performance
  - Must have write permissions

#### `max_arena_size_gb`
- **Type**: Integer (u64)
- **Required**: No
- **Default**: `100`
- **Range**: 1 to 1,000,000
- **Unit**: Gigabytes
- **Description**: Maximum size of the arena allocator
- **Example**: `max_arena_size_gb = 500`
- **Notes**:
  - Should be less than available RAM
  - Larger values allow more data in memory
  - Consider leaving 25% RAM for OS

#### `compression_enabled`
- **Type**: Boolean
- **Required**: No
- **Default**: `true`
- **Description**: Enable adaptive compression
- **Example**: `compression_enabled = true`
- **Notes**:
  - Reduces storage by 5-10x on average
  - Minimal CPU overhead
  - Recommended for production

#### `compression_threshold_kb`
- **Type**: Integer (u32)
- **Required**: No
- **Default**: `1`
- **Range**: 0 to 1,048,576
- **Unit**: Kilobytes
- **Description**: Minimum object size for compression
- **Example**: `compression_threshold_kb = 4`
- **Notes**:
  - Objects smaller than this are not compressed
  - Avoids overhead for small objects

#### `sync_mode`
- **Type**: String (Enum)
- **Required**: No
- **Default**: `"fsync"`
- **Options**: `"fsync"`, `"fdatasync"`, `"none"`
- **Description**: WAL sync mode for durability
- **Example**: `sync_mode = "fsync"`
- **Notes**:
  - `fsync`: Full durability (recommended)
  - `fdatasync`: Faster, slightly less durable
  - `none`: No sync (testing only, data loss possible)

---

## Replication Configuration

### `[replication]` Section

#### `replication_factor`
- **Type**: Integer (usize)
- **Required**: No
- **Default**: `3`
- **Range**: 1 to 100
- **Description**: Number of replicas for each piece of data
- **Example**: `replication_factor = 3`
- **Notes**:
  - 1 = single-node (no replication)
  - 3 = recommended for production
  - Must be ≤ number of nodes

#### `election_timeout_ms`
- **Type**: Integer (u64)
- **Required**: No
- **Default**: `2000`
- **Range**: 100 to 60,000
- **Unit**: Milliseconds
- **Description**: Raft election timeout
- **Example**: `election_timeout_ms = 2000`
- **Notes**:
  - Higher values = more stable, slower failover
  - Lower values = faster failover, more elections
  - Should be 10x heartbeat interval

#### `heartbeat_interval_ms`
- **Type**: Integer (u64)
- **Required**: No
- **Default**: `500`
- **Range**: 10 to 10,000
- **Unit**: Milliseconds
- **Description**: Raft heartbeat interval
- **Example**: `heartbeat_interval_ms = 500`
- **Notes**:
  - Leader sends heartbeats at this interval
  - Should be much less than election timeout
  - Lower values = faster failure detection

#### `snapshot_interval_entries`
- **Type**: Integer (u64)
- **Required**: No
- **Default**: `10000`
- **Range**: 100 to 1,000,000
- **Description**: Create snapshot after this many log entries
- **Example**: `snapshot_interval_entries = 10000`
- **Notes**:
  - Prevents unbounded log growth
  - Higher values = less frequent snapshots
  - Lower values = more frequent snapshots

---

## Performance Configuration

### `[performance]` Section

#### `max_connections`
- **Type**: Integer (usize)
- **Required**: No
- **Default**: `1000`
- **Range**: 1 to 1,000,000
- **Description**: Maximum concurrent client connections
- **Example**: `max_connections = 5000`
- **Notes**:
  - Limited by system file descriptors
  - Increase with `ulimit -n`
  - Each connection uses ~1MB RAM

#### `query_timeout_ms`
- **Type**: Integer (u64)
- **Required**: No
- **Default**: `30000`
- **Range**: 100 to 3,600,000
- **Unit**: Milliseconds
- **Description**: Maximum query execution time
- **Example**: `query_timeout_ms = 60000`
- **Notes**:
  - Queries exceeding this are cancelled
  - Prevents runaway queries
  - Adjust based on workload

#### `cache_size_mb`
- **Type**: Integer (usize)
- **Required**: No
- **Default**: `1024`
- **Range**: 1 to 1,000,000
- **Unit**: Megabytes
- **Description**: Size of object cache
- **Example**: `cache_size_mb = 8192`
- **Notes**:
  - Larger cache = better performance
  - Should be less than available RAM
  - LRU eviction policy

#### `worker_threads`
- **Type**: Integer (usize)
- **Required**: No
- **Default**: Number of CPU cores
- **Range**: 1 to 1024
- **Description**: Number of worker threads for query execution
- **Example**: `worker_threads = 16`
- **Notes**:
  - Default is optimal for most workloads
  - More threads = more parallelism
  - Diminishing returns beyond CPU cores

#### `batch_size`
- **Type**: Integer (usize)
- **Required**: No
- **Default**: `1000`
- **Range**: 1 to 1,000,000
- **Description**: Default batch size for bulk operations
- **Example**: `batch_size = 10000`
- **Notes**:
  - Larger batches = better throughput
  - Larger batches = more memory usage
  - Tune based on object size

---

## Security Configuration

### `[security]` Section

#### `auth_enabled`
- **Type**: Boolean
- **Required**: No
- **Default**: `false`
- **Description**: Enable authentication
- **Example**: `auth_enabled = true`
- **Notes**:
  - Recommended for production
  - Requires user management setup

#### `tls_enabled`
- **Type**: Boolean
- **Required**: No
- **Default**: `false`
- **Description**: Enable TLS encryption
- **Example**: `tls_enabled = true`
- **Notes**:
  - Encrypts client-server communication
  - Requires certificate configuration

#### `tls_cert_path`
- **Type**: String (Path)
- **Required**: If `tls_enabled = true`
- **Default**: None
- **Description**: Path to TLS certificate file
- **Example**: `tls_cert_path = "/etc/owl/certs/server.crt"`

#### `tls_key_path`
- **Type**: String (Path)
- **Required**: If `tls_enabled = true`
- **Default**: None
- **Description**: Path to TLS private key file
- **Example**: `tls_key_path = "/etc/owl/certs/server.key"`

#### `admin_token`
- **Type**: String
- **Required**: No
- **Default**: None
- **Description**: Admin authentication token
- **Example**: `admin_token = "your-secret-token-here"`
- **Notes**:
  - Used for admin CLI operations
  - Keep secret and secure
  - Rotate regularly

---

## Observability Configuration

### `[observability]` Section

#### `metrics_enabled`
- **Type**: Boolean
- **Required**: No
- **Default**: `true`
- **Description**: Enable Prometheus metrics
- **Example**: `metrics_enabled = true`

#### `metrics_port`
- **Type**: Integer (u16)
- **Required**: No
- **Default**: `9090`
- **Range**: 1 to 65535
- **Description**: Port for metrics endpoint
- **Example**: `metrics_port = 9090`

#### `tracing_enabled`
- **Type**: Boolean
- **Required**: No
- **Default**: `false`
- **Description**: Enable distributed tracing
- **Example**: `tracing_enabled = true`

#### `tracing_endpoint`
- **Type**: String
- **Required**: If `tracing_enabled = true`
- **Default**: None
- **Description**: OpenTelemetry collector endpoint
- **Example**: `tracing_endpoint = "http://localhost:4317"`

#### `log_level`
- **Type**: String (Enum)
- **Required**: No
- **Default**: `"info"`
- **Options**: `"trace"`, `"debug"`, `"info"`, `"warn"`, `"error"`
- **Description**: Logging verbosity level
- **Example**: `log_level = "info"`

#### `log_format`
- **Type**: String (Enum)
- **Required**: No
- **Default**: `"json"`
- **Options**: `"json"`, `"text"`
- **Description**: Log output format
- **Example**: `log_format = "json"`

---

## Advanced Configuration

### `[advanced]` Section

#### `gc_interval_ms`
- **Type**: Integer (u64)
- **Required**: No
- **Default**: `60000`
- **Range**: 1000 to 3,600,000
- **Unit**: Milliseconds
- **Description**: Garbage collection interval
- **Example**: `gc_interval_ms = 30000`

#### `checkpoint_interval_ms`
- **Type**: Integer (u64)
- **Required**: No
- **Default**: `300000`
- **Range**: 10000 to 3,600,000
- **Unit**: Milliseconds
- **Description**: Checkpoint creation interval
- **Example**: `checkpoint_interval_ms = 600000`

#### `prefetch_enabled`
- **Type**: Boolean
- **Required**: No
- **Default**: `true`
- **Description**: Enable memory prefetching
- **Example**: `prefetch_enabled = true`

#### `simd_enabled`
- **Type**: Boolean
- **Required**: No
- **Default**: `true`
- **Description**: Enable SIMD optimizations
- **Example**: `simd_enabled = true`
- **Notes**:
  - Requires CPU with AVX2 support
  - Significant performance improvement

---

## Environment Variables

All configuration options can be overridden with environment variables using the format:

```
OWL_<SECTION>_<KEY>
```

### Examples

```bash
# Cluster settings
export OWL_CLUSTER_NODE_ID=1
export OWL_CLUSTER_LISTEN_ADDRESS="0.0.0.0:8080"

# Storage settings
export OWL_STORAGE_DATA_DIR="/var/lib/owl/data"
export OWL_STORAGE_COMPRESSION_ENABLED=true

# Performance settings
export OWL_PERFORMANCE_MAX_CONNECTIONS=5000
export OWL_PERFORMANCE_CACHE_SIZE_MB=8192

# Observability settings
export OWL_OBSERVABILITY_LOG_LEVEL=debug
```

---

## Configuration Examples

### Development (Single-Node)

```toml
[cluster]
node_id = 1
listen_address = "127.0.0.1:8080"
peers = []

[storage]
data_dir = "./data"
wal_dir = "./wal"
max_arena_size_gb = 10
compression_enabled = false

[replication]
replication_factor = 1

[performance]
max_connections = 100
cache_size_mb = 512

[observability]
log_level = "debug"
log_format = "text"
```

### Production (3-Node Cluster)

```toml
[cluster]
node_id = 1
listen_address = "0.0.0.0:8080"
peers = ["node2.prod.example.com:8080", "node3.prod.example.com:8080"]

[storage]
data_dir = "/var/lib/owl/data"
wal_dir = "/var/lib/owl/wal"
max_arena_size_gb = 500
compression_enabled = true
sync_mode = "fsync"

[replication]
replication_factor = 3
election_timeout_ms = 2000
heartbeat_interval_ms = 500

[performance]
max_connections = 10000
query_timeout_ms = 30000
cache_size_mb = 16384
worker_threads = 32

[security]
auth_enabled = true
tls_enabled = true
tls_cert_path = "/etc/owl/certs/server.crt"
tls_key_path = "/etc/owl/certs/server.key"

[observability]
metrics_enabled = true
metrics_port = 9090
tracing_enabled = true
tracing_endpoint = "http://otel-collector:4317"
log_level = "info"
log_format = "json"
```

### High-Performance (Large Cluster)

```toml
[cluster]
node_id = 1
listen_address = "0.0.0.0:8080"
peers = ["node2:8080", "node3:8080", "node4:8080", "node5:8080"]

[storage]
data_dir = "/nvme/owl/data"
wal_dir = "/nvme/owl/wal"
max_arena_size_gb = 1000
compression_enabled = true
compression_threshold_kb = 4

[replication]
replication_factor = 3
election_timeout_ms = 1500
heartbeat_interval_ms = 300

[performance]
max_connections = 50000
query_timeout_ms = 60000
cache_size_mb = 65536
worker_threads = 64
batch_size = 10000

[advanced]
gc_interval_ms = 30000
checkpoint_interval_ms = 300000
prefetch_enabled = true
simd_enabled = true
```

---

## Configuration Validation

### Validate Configuration

```bash
# Validate configuration file
owl-admin config validate /etc/owl/owl.toml

# Show current configuration
owl-admin config show

# Get specific value
owl-admin config get cluster.node_id

# Set specific value
owl-admin config set performance.max_connections 10000
```

---

## Best Practices

1. **Always validate** configuration before restarting
2. **Use version control** for configuration files
3. **Document changes** in comments
4. **Test in staging** before production
5. **Monitor metrics** after configuration changes
6. **Keep backups** of working configurations

---

**Last Updated**: November 26, 2025  
**Document Version**: 1.0  
**Owl Database Version**: 1.0.0+
