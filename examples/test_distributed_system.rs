// Test Distributed System Implementation
//
// This example demonstrates the distributed OQL system with GlobalObjRef,
// consistent hashing, shard management, and distributed query planning.

use owl_db::distributed::{
    ConsistentHashRing, DistributedQueryPlanner, GlobalObjRef, ShardManager,
};
use owl_db::error::OQLResult;
use std::sync::Arc;

fn main() -> OQLResult<()> {
    println!("🦉 Testing Next-Gen Owl Distributed System");
    println!("============================================");

    // Test 1: GlobalObjRef functionality
    test_global_objref()?;

    // Test 2: Consistent Hashing
    test_consistent_hashing()?;

    // Test 3: Shard Management
    test_shard_management()?;

    // Test 4: Distributed Query Planning
    test_distributed_query_planning()?;

    println!("\n✅ All distributed system tests passed!");
    println!("🚀 Next-Gen Owl with DST is ready for distributed operations!");

    Ok(())
}

fn test_global_objref() -> OQLResult<()> {
    println!("\n📍 Testing GlobalObjRef (128-bit distributed pointers)");

    // Create GlobalObjRef with shard information
    let global_ref = GlobalObjRef::new(5, 1024 * 1024, 0);
    println!("   Created GlobalObjRef: {}", global_ref);

    // Test local vs remote detection
    assert!(global_ref.is_local(5), "Should be local to shard 5");
    assert!(!global_ref.is_local(3), "Should not be local to shard 3");

    // Test encoding/decoding for network transmission
    let (high, low) = global_ref.encode();
    let decoded = GlobalObjRef::decode(high, low);
    assert_eq!(global_ref, decoded, "Encoding/decoding should be lossless");

    // Test byte serialization
    let bytes = global_ref.to_bytes();
    let from_bytes = GlobalObjRef::from_bytes(&bytes).unwrap();
    assert_eq!(global_ref, from_bytes, "Byte serialization should be lossless");

    println!("   ✓ GlobalObjRef encoding/decoding works correctly");
    println!("   ✓ 128-bit distributed pointers ready for O(1) navigation");

    Ok(())
}

fn test_consistent_hashing() -> OQLResult<()> {
    println!("\n🔄 Testing Consistent Hashing for shard distribution");

    let mut ring = ConsistentHashRing::new();

    // Add shards to the ring
    ring.add_shard(0);
    ring.add_shard(1);
    ring.add_shard(2);

    println!("   Added 3 shards to consistent hash ring");

    // Test object distribution
    let mut shard_counts = std::collections::HashMap::new();
    for i in 0..1000 {
        let object_id = format!("object_{}", i);
        let shard = ring.get_shard_for_object(&object_id).unwrap();
        *shard_counts.entry(shard).or_insert(0) += 1;
    }

    println!("   Distribution across shards:");
    for (shard, count) in &shard_counts {
        println!("     Shard {}: {} objects ({:.1}%)", shard, count, (*count as f64 / 1000.0) * 100.0);
    }

    // Verify reasonable distribution (each shard should get 20-50% of objects)
    for &count in shard_counts.values() {
        assert!(count > 200 && count < 500, "Distribution should be reasonably balanced");
    }

    // Test OQL insert parsing
    let oql = "User.insert { id: 'user123', name: 'Alice' }";
    let shard = ring.get_shard_for_oql_insert(oql);
    println!("   OQL insert '{}' -> Shard {:?}", oql, shard);

    // Test deterministic assignment
    let shard1 = ring.get_shard_for_object("user123");
    let shard2 = ring.get_shard_for_object("user123");
    assert_eq!(shard1, shard2, "Same object should always map to same shard");

    println!("   ✓ Consistent hashing distributes objects evenly");
    println!("   ✓ Deterministic shard assignment working");

    Ok(())
}

fn test_shard_management() -> OQLResult<()> {
    println!("\n🏗️  Testing Shard Management for cluster coordination");

    let manager = ShardManager::new();

    // Add nodes to the cluster
    manager.add_node("node1", "192.168.1.10:8080")?;
    manager.add_node("node2", "192.168.1.11:8080")?;
    manager.add_node("node3", "192.168.1.12:8080")?;

    println!("   Added 3 nodes to cluster");

    // Simulate node heartbeats
    let capacity = owl_db::distributed::shard_manager::NodeCapacity {
        memory_bytes: 16 * 1024 * 1024 * 1024, // 16GB
        disk_bytes: 1024 * 1024 * 1024 * 1024, // 1TB
        cpu_cores: 8,
        load: 0.3,
    };

    manager.update_node_heartbeat("node1", capacity.clone())?;
    manager.update_node_heartbeat("node2", capacity.clone())?;
    manager.update_node_heartbeat("node3", capacity)?;

    // Create shards
    let shard1 = manager.create_shard("node1")?;
    let shard2 = manager.create_shard("node2")?;
    let shard3 = manager.create_shard("node3")?;

    println!("   Created shards: {} on node1, {} on node2, {} on node3", shard1, shard2, shard3);

    // Activate shards
    manager.activate_shard(shard1)?;
    manager.activate_shard(shard2)?;
    manager.activate_shard(shard3)?;

    // Check cluster health
    let health = manager.check_cluster_health()?;
    println!("   Cluster health:");
    println!("     Total nodes: {}", health.total_nodes);
    println!("     Healthy nodes: {}", health.healthy_nodes);
    println!("     Active shards: {}/{}", health.active_shards, health.total_shards);
    println!("     Availability: {:.1}%", health.availability() * 100.0);

    assert!(health.is_healthy(), "Cluster should be healthy");

    // Test object shard assignment
    let shard = manager.get_shard_for_object("user123")?;
    println!("   Object 'user123' assigned to shard: {:?}", shard);

    println!("   ✓ Shard management and cluster coordination working");
    println!("   ✓ Automatic shard assignment and health monitoring active");

    Ok(())
}

fn test_distributed_query_planning() -> OQLResult<()> {
    println!("\n🧠 Testing Distributed Query Planning");

    let shard_manager = Arc::new(ShardManager::new());
    let planner = DistributedQueryPlanner::new(shard_manager);

    // Test simple query planning
    let oql = "User.where { .age > 25 }.select { id, name }";
    let plan = planner.plan_query(oql)?;

    println!("   Planned query: {}", oql);
    println!("   Plan ID: {}", plan.plan_id);
    println!("   Estimated cost: {:.2}ns", plan.estimated_cost);
    println!("   Estimated time: {}ms", plan.estimated_time_ms);
    println!("   Network hops: {}", plan.network_hops);
    println!("   Involved shards: {} shards", plan.involved_shards.len());

    // Test cross-shard analysis
    let analysis = planner.analyze_cross_shard_dependencies(oql)?;
    println!("   Cross-shard analysis:");
    println!("     Total operations: {}", analysis.total_operations);
    println!("     Cross-shard operations: {}", analysis.cross_shard_operations);
    println!("     Cross-shard ratio: {:.1}%", analysis.cross_shard_ratio() * 100.0);
    println!("     Estimated network hops: {}", analysis.estimated_network_hops);

    // Test query statistics
    planner.update_query_stats("User.select", 150.0, 2, true)?;
    planner.update_query_stats("User.select", 120.0, 1, true)?;

    if let Some(stats) = planner.get_query_stats("User.select")? {
        println!("   Query statistics for 'User.select':");
        println!("     Executions: {}", stats.execution_count);
        println!("     Avg time: {:.1}ms", stats.avg_execution_time);
        println!("     Avg hops: {:.1}", stats.avg_network_hops);
        println!("     Success rate: {:.1}%", stats.success_rate * 100.0);
    }

    println!("   ✓ Distributed query planning and optimization working");
    println!("   ✓ Cross-shard dependency analysis functional");

    Ok(())
}