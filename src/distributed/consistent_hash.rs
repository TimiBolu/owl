// Consistent Hashing for OQL Shard Distribution
//
// This module implements consistent hashing with virtual nodes for smooth rebalancing.
// Uses Wyhash for speed and provides OQL-aware object placement.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Number of virtual nodes per physical shard (default: 256)
/// Higher values provide better distribution but use more memory
pub const DEFAULT_VNODES_PER_SHARD: u32 = 256;

/// Shard identifier (0 to 4 billion shards supported)
pub type ShardId = u32;

/// Virtual node in the consistent hash ring
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct VirtualNode {
    /// Hash value for this virtual node
    hash: u64,
    /// Physical shard that owns this virtual node
    shard_id: ShardId,
    /// Virtual node index within the shard (0 to vnodes_per_shard-1)
    vnode_index: u32,
}

impl PartialOrd for VirtualNode {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for VirtualNode {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.hash.cmp(&other.hash)
    }
}

/// Consistent hash ring for distributing OQL objects across shards
///
/// Features:
/// - Virtual nodes for smooth rebalancing (adding N nodes moves only 1/N of data)
/// - Wyhash for fast hash computation
/// - OQL-aware placement (co-locate related objects)
/// - Deterministic shard assignment
///
/// # Example
///
/// ```
/// use oql::distributed::ConsistentHashRing;
///
/// let mut ring = ConsistentHashRing::new();
/// ring.add_shard(0);
/// ring.add_shard(1);
/// ring.add_shard(2);
///
/// // Distribute objects
/// let shard = ring.get_shard_for_object("user123");
/// let schema_shard = ring.get_shard_for_schema("User");
/// ```
pub struct ConsistentHashRing {
    /// Virtual nodes sorted by hash value for binary search
    vnodes: BTreeMap<u64, VirtualNode>,
    /// Number of virtual nodes per physical shard
    vnodes_per_shard: u32,
    /// Set of active shards
    active_shards: std::collections::HashSet<ShardId>,
}

impl ConsistentHashRing {
    /// Create a new empty hash ring
    pub fn new() -> Self {
        Self::with_vnodes_per_shard(DEFAULT_VNODES_PER_SHARD)
    }

    /// Create a new hash ring with custom virtual nodes per shard
    pub fn with_vnodes_per_shard(vnodes_per_shard: u32) -> Self {
        Self {
            vnodes: BTreeMap::new(),
            vnodes_per_shard,
            active_shards: std::collections::HashSet::new(),
        }
    }

    /// Add a shard to the ring
    ///
    /// This creates virtual nodes for the shard and distributes them around the ring.
    /// Adding a shard triggers automatic rebalancing.
    ///
    /// # Arguments
    /// * `shard_id` - Unique identifier for the shard
    ///
    /// # Example
    /// ```
    /// # use oql::distributed::ConsistentHashRing;
    /// let mut ring = ConsistentHashRing::new();
    /// ring.add_shard(0);
    /// ring.add_shard(1);
    /// ```
    pub fn add_shard(&mut self, shard_id: ShardId) {
        if self.active_shards.contains(&shard_id) {
            return; // Shard already exists
        }

        // Add virtual nodes for this shard
        for vnode_index in 0..self.vnodes_per_shard {
            let hash = self.hash_vnode(shard_id, vnode_index);
            let vnode = VirtualNode {
                hash,
                shard_id,
                vnode_index,
            };
            self.vnodes.insert(hash, vnode);
        }

        self.active_shards.insert(shard_id);
    }

    /// Remove a shard from the ring
    ///
    /// This removes all virtual nodes for the shard and triggers rebalancing.
    /// Data on the removed shard needs to be migrated to other shards.
    ///
    /// # Arguments
    /// * `shard_id` - Identifier of the shard to remove
    ///
    /// # Returns
    /// * `true` if the shard was removed
    /// * `false` if the shard was not in the ring
    pub fn remove_shard(&mut self, shard_id: ShardId) -> bool {
        if !self.active_shards.contains(&shard_id) {
            return false;
        }

        // Remove all virtual nodes for this shard
        let mut to_remove = Vec::new();
        for (&hash, &vnode) in &self.vnodes {
            if vnode.shard_id == shard_id {
                to_remove.push(hash);
            }
        }

        for hash in to_remove {
            self.vnodes.remove(&hash);
        }

        self.active_shards.remove(&shard_id);
        true
    }

    /// Get the shard responsible for an OQL object by ID
    ///
    /// Uses Wyhash for fast computation and consistent results.
    ///
    /// # Arguments
    /// * `object_id` - String identifier of the object
    ///
    /// # Returns
    /// * `Some(shard_id)` if there are active shards
    /// * `None` if no shards are available
    ///
    /// # Example
    /// ```
    /// # use oql::distributed::ConsistentHashRing;
    /// # let mut ring = ConsistentHashRing::new();
    /// # ring.add_shard(0);
    /// let shard = ring.get_shard_for_object("user123").unwrap();
    /// ```
    pub fn get_shard_for_object(&self, object_id: &str) -> Option<ShardId> {
        if self.vnodes.is_empty() {
            return None;
        }

        let hash = self.hash_string(object_id);
        let vnode = self.find_next_vnode(hash);
        Some(vnode.shard_id)
    }

    /// Get the shard for an OQL schema type (for co-location)
    ///
    /// This can be used to co-locate related objects by placing them
    /// on the same shard as their schema.
    ///
    /// # Arguments
    /// * `schema_name` - Name of the OQL schema
    ///
    /// # Returns
    /// * `Some(shard_id)` if there are active shards
    /// * `None` if no shards are available
    pub fn get_shard_for_schema(&self, schema_name: &str) -> Option<ShardId> {
        self.get_shard_for_object(schema_name)
    }

    /// Get shard for OQL insert operation
    ///
    /// Parses the OQL insert statement to extract object ID and determine shard.
    /// This is a convenience method for the query planner.
    ///
    /// # Arguments
    /// * `oql_insert` - OQL insert statement
    ///
    /// # Returns
    /// * `Some(shard_id)` if parsing succeeds and shards are available
    /// * `None` if parsing fails or no shards available
    ///
    /// # Example
    /// ```
    /// # use oql::distributed::ConsistentHashRing;
    /// # let mut ring = ConsistentHashRing::new();
    /// # ring.add_shard(0);
    /// let oql = "User.insert { id: 'user123', name: 'Alice' }";
    /// let shard = ring.get_shard_for_oql_insert(oql);
    /// ```
    pub fn get_shard_for_oql_insert(&self, oql_insert: &str) -> Option<ShardId> {
        // Simple parsing to extract object ID from OQL insert
        // This is a basic implementation - production would use proper OQL parser

        // Look for id: 'value' or id: "value" pattern
        if let Some(id_start) = oql_insert.find("id:") {
            let after_id = &oql_insert[id_start + 3..];

            // Skip whitespace
            let after_id = after_id.trim_start();

            // Extract quoted string
            if let Some(quote_char) = after_id.chars().next() {
                if quote_char == '\'' || quote_char == '"' {
                    if let Some(end_quote) = after_id[1..].find(quote_char) {
                        let object_id = &after_id[1..end_quote + 1];
                        return self.get_shard_for_object(object_id);
                    }
                }
            }
        }

        None
    }

    /// List all active shards
    pub fn active_shards(&self) -> Vec<ShardId> {
        self.active_shards.iter().copied().collect()
    }

    /// Get the number of active shards
    pub fn shard_count(&self) -> usize {
        self.active_shards.len()
    }

    /// Get distribution statistics
    ///
    /// Returns information about how evenly data is distributed across shards.
    /// Useful for monitoring and rebalancing decisions.
    pub fn distribution_stats(&self) -> DistributionStats {
        let total_vnodes = self.vnodes.len();
        let shard_count = self.active_shards.len();

        if shard_count == 0 {
            return DistributionStats {
                total_vnodes: 0,
                shards: 0,
                vnodes_per_shard: 0,
                min_vnodes: 0,
                max_vnodes: 0,
                variance: 0.0,
            };
        }

        // Count vnodes per shard
        let mut vnode_counts = std::collections::HashMap::new();
        for vnode in self.vnodes.values() {
            *vnode_counts.entry(vnode.shard_id).or_insert(0) += 1;
        }

        let counts: Vec<usize> = vnode_counts.values().copied().collect();
        let min_vnodes = *counts.iter().min().unwrap_or(&0);
        let max_vnodes = *counts.iter().max().unwrap_or(&0);

        // Calculate variance
        let mean = total_vnodes as f64 / shard_count as f64;
        let variance = counts
            .iter()
            .map(|&count| {
                let diff = count as f64 - mean;
                diff * diff
            })
            .sum::<f64>()
            / shard_count as f64;

        DistributionStats {
            total_vnodes,
            shards: shard_count,
            vnodes_per_shard: self.vnodes_per_shard as usize,
            min_vnodes,
            max_vnodes,
            variance,
        }
    }

    /// Hash a virtual node (shard_id, vnode_index) -> u64
    fn hash_vnode(&self, shard_id: ShardId, vnode_index: u32) -> u64 {
        // Use Wyhash for speed - same as Zig std.hash.Wyhash
        let mut hasher = wyhash::WyHash::with_seed(0);
        shard_id.hash(&mut hasher);
        vnode_index.hash(&mut hasher);
        hasher.finish()
    }

    /// Hash a string using Wyhash
    fn hash_string(&self, s: &str) -> u64 {
        let mut hasher = wyhash::WyHash::with_seed(0);
        s.hash(&mut hasher);
        hasher.finish()
    }

    /// Find the next virtual node in the ring (clockwise)
    fn find_next_vnode(&self, hash: u64) -> VirtualNode {
        // Find first vnode with hash >= target
        if let Some((&_hash, &vnode)) = self.vnodes.range(hash..).next() {
            vnode
        } else {
            // Wrap around to first vnode
            self.vnodes.values().next().copied().unwrap()
        }
    }
}

impl Default for ConsistentHashRing {
    fn default() -> Self {
        Self::new()
    }
}

/// Statistics about hash ring distribution
#[derive(Debug, Clone)]
pub struct DistributionStats {
    /// Total number of virtual nodes
    pub total_vnodes: usize,
    /// Number of active shards
    pub shards: usize,
    /// Configured virtual nodes per shard
    pub vnodes_per_shard: usize,
    /// Minimum vnodes on any shard
    pub min_vnodes: usize,
    /// Maximum vnodes on any shard
    pub max_vnodes: usize,
    /// Variance in vnode distribution
    pub variance: f64,
}

impl DistributionStats {
    /// Calculate distribution uniformity (0.0 = perfectly uniform, 1.0 = maximally uneven)
    pub fn uniformity(&self) -> f64 {
        if self.shards <= 1 {
            return 1.0;
        }

        let expected_per_shard = self.total_vnodes as f64 / self.shards as f64;
        let max_deviation = (self.max_vnodes as f64 - expected_per_shard)
            .abs()
            .max((expected_per_shard - self.min_vnodes as f64).abs());

        if expected_per_shard == 0.0 {
            return 1.0;
        }

        1.0 - (max_deviation / expected_per_shard).min(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_ring() {
        let ring = ConsistentHashRing::new();
        assert_eq!(ring.shard_count(), 0);
        assert!(ring.get_shard_for_object("test").is_none());
    }

    #[test]
    fn test_single_shard() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);

        assert_eq!(ring.shard_count(), 1);
        assert_eq!(ring.get_shard_for_object("test"), Some(0));
        assert_eq!(ring.get_shard_for_object("another"), Some(0));
    }

    #[test]
    fn test_multiple_shards() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);
        ring.add_shard(1);
        ring.add_shard(2);

        assert_eq!(ring.shard_count(), 3);

        // All objects should map to some shard
        let shard1 = ring.get_shard_for_object("user1").unwrap();
        let shard2 = ring.get_shard_for_object("user2").unwrap();
        let shard3 = ring.get_shard_for_object("user3").unwrap();

        assert!(shard1 < 3);
        assert!(shard2 < 3);
        assert!(shard3 < 3);
    }

    #[test]
    fn test_deterministic_assignment() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);
        ring.add_shard(1);

        // Same object should always map to same shard
        let shard1 = ring.get_shard_for_object("user123");
        let shard2 = ring.get_shard_for_object("user123");
        assert_eq!(shard1, shard2);
    }

    #[test]
    fn test_shard_removal() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);
        ring.add_shard(1);
        ring.add_shard(2);

        assert_eq!(ring.shard_count(), 3);

        let removed = ring.remove_shard(1);
        assert!(removed);
        assert_eq!(ring.shard_count(), 2);

        // Removing non-existent shard should return false
        let removed = ring.remove_shard(1);
        assert!(!removed);
    }

    #[test]
    fn test_distribution_uniformity() {
        let mut ring = ConsistentHashRing::with_vnodes_per_shard(100);
        ring.add_shard(0);
        ring.add_shard(1);
        ring.add_shard(2);

        // Test many objects to check distribution
        let mut shard_counts = std::collections::HashMap::new();
        for i in 0..1000 {
            let object_id = format!("object{}", i);
            let shard = ring.get_shard_for_object(&object_id).unwrap();
            *shard_counts.entry(shard).or_insert(0) += 1;
        }

        // Each shard should get roughly 1000/3 = 333 objects
        // Allow some variance but should be reasonably balanced
        for &count in shard_counts.values() {
            assert!(
                count > 250 && count < 450,
                "Shard got {} objects, expected ~333",
                count
            );
        }
    }

    #[test]
    fn test_oql_insert_parsing() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);
        ring.add_shard(1);

        // Test single quotes
        let oql1 = "User.insert { id: 'user123', name: 'Alice' }";
        let shard1 = ring.get_shard_for_oql_insert(oql1);
        assert!(shard1.is_some());

        // Test double quotes
        let oql2 = "User.insert { id: \"user456\", name: \"Bob\" }";
        let shard2 = ring.get_shard_for_oql_insert(oql2);
        assert!(shard2.is_some());

        // Same ID should map to same shard
        let oql3 = "User.insert { id: 'user123', email: 'alice@example.com' }";
        let shard3 = ring.get_shard_for_oql_insert(oql3);
        assert_eq!(shard1, shard3);
    }

    #[test]
    fn test_schema_placement() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);
        ring.add_shard(1);

        let user_shard = ring.get_shard_for_schema("User");
        let order_shard = ring.get_shard_for_schema("Order");

        assert!(user_shard.is_some());
        assert!(order_shard.is_some());

        // Same schema should always map to same shard
        let user_shard2 = ring.get_shard_for_schema("User");
        assert_eq!(user_shard, user_shard2);
    }

    #[test]
    fn test_distribution_stats() {
        let mut ring = ConsistentHashRing::with_vnodes_per_shard(10);
        ring.add_shard(0);
        ring.add_shard(1);

        let stats = ring.distribution_stats();
        assert_eq!(stats.total_vnodes, 20);
        assert_eq!(stats.shards, 2);
        assert_eq!(stats.vnodes_per_shard, 10);

        // Should be perfectly balanced
        assert_eq!(stats.min_vnodes, 10);
        assert_eq!(stats.max_vnodes, 10);
        assert_eq!(stats.variance, 0.0);
        assert_eq!(stats.uniformity(), 1.0);
    }

    #[test]
    fn test_rebalancing_minimal_movement() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);
        ring.add_shard(1);

        // Record initial assignments
        let mut initial_assignments = std::collections::HashMap::new();
        for i in 0..1000 {
            let object_id = format!("object{}", i);
            let shard = ring.get_shard_for_object(&object_id).unwrap();
            initial_assignments.insert(object_id, shard);
        }

        // Add a third shard
        ring.add_shard(2);

        // Count how many objects moved
        let mut moved = 0;
        for i in 0..1000 {
            let object_id = format!("object{}", i);
            let new_shard = ring.get_shard_for_object(&object_id).unwrap();
            let old_shard = initial_assignments[&object_id];
            if new_shard != old_shard {
                moved += 1;
            }
        }

        // Should move roughly 1/3 of objects (adding 1 shard to 2 existing)
        // Allow some variance due to hash distribution
        assert!(
            moved > 250 && moved < 450,
            "Moved {} objects, expected ~333",
            moved
        );
    }

    #[test]
    fn test_duplicate_shard_addition() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(0);
        ring.add_shard(0); // Duplicate

        assert_eq!(ring.shard_count(), 1);
    }

    #[test]
    fn test_active_shards_list() {
        let mut ring = ConsistentHashRing::new();
        ring.add_shard(2);
        ring.add_shard(0);
        ring.add_shard(1);

        let mut shards = ring.active_shards();
        shards.sort();
        assert_eq!(shards, vec![0, 1, 2]);
    }
}
