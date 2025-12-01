# Owl Database Installation Guide

## Overview

This guide provides step-by-step instructions for installing and configuring Owl Database in various deployment scenarios.

## Table of Contents

1. [System Requirements](#system-requirements)
2. [Installation Methods](#installation-methods)
3. [Single-Node Installation](#single-node-installation)
4. [Distributed Cluster Installation](#distributed-cluster-installation)
5. [Configuration](#configuration)
6. [Verification](#verification)
7. [Next Steps](#next-steps)

---

## System Requirements

### Minimum Requirements

- **CPU**: 4 cores (x86_64 or ARM64)
- **RAM**: 8 GB
- **Disk**: 50 GB SSD (NVMe recommended)
- **OS**: Linux (Ubuntu 20.04+, RHEL 8+, Debian 11+)
- **Network**: 1 Gbps

### Recommended Requirements (Production)

- **CPU**: 16+ cores (x86_64 with AVX2 support)
- **RAM**: 64 GB+
- **Disk**: 500 GB+ NVMe SSD
- **OS**: Linux (Ubuntu 22.04 LTS or RHEL 9)
- **Network**: 10 Gbps with low latency (<1ms between nodes)

### Software Dependencies

- **Zig**: 0.11.0+ (for building from source)
- **Rust**: 1.70.0+ (for building from source)
- **systemd**: For service management
- **Optional**: Docker 20.10+ (for containerized deployment)

---

## Installation Methods

### Method 1: Binary Installation (Recommended)

Download pre-built binaries from the official releases:

```bash
# Download latest release
wget https://releases.owl-db.io/latest/owl-linux-amd64.tar.gz

# Extract
tar -xzf owl-linux-amd64.tar.gz

# Install
sudo mv owl /usr/local/bin/
sudo mv owl-admin /usr/local/bin/
sudo chmod +x /usr/local/bin/owl
sudo chmod +x /usr/local/bin/owl-admin

# Verify installation
owl --version
owl-admin --version
```

### Method 2: Package Manager Installation

#### Ubuntu/Debian

```bash
# Add Owl repository
curl -fsSL https://packages.owl-db.io/gpg | sudo gpg --dearmor -o /usr/share/keyrings/owl-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/owl-archive-keyring.gpg] https://packages.owl-db.io/apt stable main" | sudo tee /etc/apt/sources.list.d/owl.list

# Install
sudo apt update
sudo apt install owl-db
```

#### RHEL/CentOS

```bash
# Add Owl repository
sudo tee /etc/yum.repos.d/owl.repo <<EOF
[owl]
name=Owl Database
baseurl=https://packages.owl-db.io/rpm/stable
enabled=1
gpgcheck=1
gpgkey=https://packages.owl-db.io/gpg
EOF

# Install
sudo yum install owl-db
```

### Method 3: Docker Installation

```bash
# Pull official image
docker pull owldb/owl:latest

# Run single-node instance
docker run -d \
  --name owl \
  -p 8080:8080 \
  -v /var/lib/owl:/data \
  owldb/owl:latest
```

### Method 4: Build from Source

```bash
# Clone repository
git clone https://github.com/owl-db/owl.git
cd owl

# Install dependencies
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
# Install Zig from https://ziglang.org/download/

# Build
cargo build --release

# Install
sudo cp target/release/owl /usr/local/bin/
sudo cp target/release/owl-admin /usr/local/bin/
```

---

## Single-Node Installation

Perfect for development, testing, or small deployments.

### Step 1: Create User and Directories

```bash
# Create owl user
sudo useradd -r -s /bin/false owl

# Create directories
sudo mkdir -p /var/lib/owl/{data,wal,backups}
sudo mkdir -p /etc/owl
sudo mkdir -p /var/log/owl

# Set permissions
sudo chown -R owl:owl /var/lib/owl
sudo chown -R owl:owl /var/log/owl
```

### Step 2: Create Configuration

```bash
sudo tee /etc/owl/owl.toml <<EOF
[cluster]
node_id = 1
listen_address = "0.0.0.0:8080"
peers = []

[storage]
data_dir = "/var/lib/owl/data"
wal_dir = "/var/lib/owl/wal"
max_arena_size_gb = 100
compression_enabled = true

[replication]
replication_factor = 1
election_timeout_ms = 2000
heartbeat_interval_ms = 500

[performance]
max_connections = 1000
query_timeout_ms = 30000
cache_size_mb = 1024
EOF
```

### Step 3: Create systemd Service

```bash
sudo tee /etc/systemd/system/owl.service <<EOF
[Unit]
Description=Owl Database
After=network.target

[Service]
Type=simple
User=owl
Group=owl
ExecStart=/usr/local/bin/owl --config /etc/owl/owl.toml
Restart=on-failure
RestartSec=5
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF
```

### Step 4: Start Service

```bash
# Reload systemd
sudo systemctl daemon-reload

# Enable service
sudo systemctl enable owl

# Start service
sudo systemctl start owl

# Check status
sudo systemctl status owl
```

### Step 5: Verify Installation

```bash
# Check service is running
owl-admin health check

# Run smoke test
owl-admin test smoke

# Check logs
sudo journalctl -u owl -n 50
```

---

## Distributed Cluster Installation

For production deployments requiring high availability and horizontal scaling.

### Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────────┐
│   Node 1    │────▶│   Node 2    │────▶│   Node 3    │
│  (Leader)   │◀────│ (Follower)  │◀────│ (Follower)  │
└─────────────┘     └─────────────┘     └─────────────┘
```

### Prerequisites

- 3+ nodes (odd number recommended for Raft consensus)
- Network connectivity between all nodes
- Synchronized clocks (NTP recommended)
- Same Owl version on all nodes

### Step 1: Prepare All Nodes

On each node, complete the single-node installation steps 1-3, but use different configurations.

### Step 2: Configure Node 1 (Initial Leader)

```bash
sudo tee /etc/owl/owl.toml <<EOF
[cluster]
node_id = 1
listen_address = "0.0.0.0:8080"
peers = ["node2.example.com:8080", "node3.example.com:8080"]

[storage]
data_dir = "/var/lib/owl/data"
wal_dir = "/var/lib/owl/wal"
max_arena_size_gb = 500
compression_enabled = true

[replication]
replication_factor = 3
election_timeout_ms = 2000
heartbeat_interval_ms = 500

[performance]
max_connections = 5000
query_timeout_ms = 30000
cache_size_mb = 8192
EOF
```

### Step 3: Configure Node 2

```bash
sudo tee /etc/owl/owl.toml <<EOF
[cluster]
node_id = 2
listen_address = "0.0.0.0:8080"
peers = ["node1.example.com:8080", "node3.example.com:8080"]

[storage]
data_dir = "/var/lib/owl/data"
wal_dir = "/var/lib/owl/wal"
max_arena_size_gb = 500
compression_enabled = true

[replication]
replication_factor = 3
election_timeout_ms = 2000
heartbeat_interval_ms = 500

[performance]
max_connections = 5000
query_timeout_ms = 30000
cache_size_mb = 8192
EOF
```

### Step 4: Configure Node 3

```bash
sudo tee /etc/owl/owl.toml <<EOF
[cluster]
node_id = 3
listen_address = "0.0.0.0:8080"
peers = ["node1.example.com:8080", "node2.example.com:8080"]

[storage]
data_dir = "/var/lib/owl/data"
wal_dir = "/var/lib/owl/wal"
max_arena_size_gb = 500
compression_enabled = true

[replication]
replication_factor = 3
election_timeout_ms = 2000
heartbeat_interval_ms = 500

[performance]
max_connections = 5000
query_timeout_ms = 30000
cache_size_mb = 8192
EOF
```

### Step 5: Start Cluster

```bash
# Start Node 1 first (will become initial leader)
ssh node1 "sudo systemctl start owl"

# Wait 5 seconds
sleep 5

# Start Node 2
ssh node2 "sudo systemctl start owl"

# Wait 5 seconds
sleep 5

# Start Node 3
ssh node3 "sudo systemctl start owl"
```

### Step 6: Verify Cluster

```bash
# Check cluster status
owl-admin cluster status

# List all nodes
owl-admin cluster nodes

# Check replication
owl-admin health replication

# Expected output:
# Total Nodes: 3
# Healthy Nodes: 3
# Leader: Node 1 (node1.example.com:8080)
# Cluster Health: Excellent
```

---

## Configuration

### Configuration File Format

Owl uses TOML format for configuration. The default location is `/etc/owl/owl.toml`.

### Configuration Sections

#### Cluster Configuration

```toml
[cluster]
# Unique node identifier (1-4294967295)
node_id = 1

# Address to listen on (host:port)
listen_address = "0.0.0.0:8080"

# List of peer nodes (for distributed mode)
peers = ["node2:8080", "node3:8080"]
```

#### Storage Configuration

```toml
[storage]
# Directory for data files
data_dir = "/var/lib/owl/data"

# Directory for WAL files
wal_dir = "/var/lib/owl/wal"

# Maximum arena size in GB
max_arena_size_gb = 100

# Enable adaptive compression
compression_enabled = true
```

#### Replication Configuration

```toml
[replication]
# Number of replicas (1 for single-node, 3+ for distributed)
replication_factor = 3

# Raft election timeout in milliseconds
election_timeout_ms = 2000

# Raft heartbeat interval in milliseconds
heartbeat_interval_ms = 500
```

#### Performance Configuration

```toml
[performance]
# Maximum concurrent connections
max_connections = 1000

# Query timeout in milliseconds
query_timeout_ms = 30000

# Cache size in MB
cache_size_mb = 1024
```

### Environment Variables

Override configuration with environment variables:

```bash
# Cluster settings
export OWL_NODE_ID=1
export OWL_LISTEN_ADDRESS="0.0.0.0:8080"

# Storage settings
export OWL_DATA_DIR="/var/lib/owl/data"
export OWL_WAL_DIR="/var/lib/owl/wal"

# Performance settings
export OWL_MAX_CONNECTIONS=5000
export OWL_CACHE_SIZE_MB=8192
```

---

## Verification

### Health Check

```bash
# Check overall health
owl-admin health check

# Expected output:
# Overall Status: Healthy
# All nodes: Healthy
# P99 Latency: <1ms
```

### Smoke Tests

```bash
# Run basic functionality tests
owl-admin test smoke

# Expected output:
# ✓ Connection test passed
# ✓ Write test passed
# ✓ Read test passed
# ✓ Query test passed
# ✓ All tests passed
```

### Performance Validation

```bash
# Run performance benchmarks
owl-admin benchmark --duration 60s

# Expected output:
# Write throughput: 10M+ TPS
# Read throughput: 5M+ QPS
# P99 latency: <1ms
```

---

## Next Steps

After successful installation:

1. **Read the Operations Manual**: See [OPERATIONS.md](OPERATIONS.md)
2. **Configure Monitoring**: Set up Prometheus and Grafana
3. **Set Up Backups**: Configure automated backups
4. **Review Security**: Configure authentication and encryption
5. **Load Your Data**: Import existing data or start fresh
6. **Connect Your Application**: Use the OQL client library

---

## Troubleshooting

### Service Won't Start

```bash
# Check logs
sudo journalctl -u owl -n 100

# Common issues:
# - Port already in use: Change listen_address
# - Permission denied: Check directory permissions
# - Config error: Validate with owl-admin config validate
```

### Cluster Won't Form

```bash
# Check network connectivity
owl-admin cluster ping <node-id>

# Check firewall
sudo firewall-cmd --list-all

# Verify configuration
owl-admin config show
```

### Performance Issues

```bash
# Check resource usage
owl-admin health node <node-id>

# Review configuration
owl-admin config show performance

# Check for slow queries
sudo journalctl -u owl | grep "slow query"
```

---

## Support

- **Documentation**: https://docs.owl-db.io
- **Community Forum**: https://community.owl-db.io
- **GitHub Issues**: https://github.com/owl-db/owl/issues
- **Enterprise Support**: support@owl-db.io

---

**Last Updated**: November 26, 2025  
**Document Version**: 1.0  
**Owl Database Version**: 1.0.0+
