// Property-based test generators for Owl distributed system
// Validates: Requirements 17

use proptest::prelude::*;
use owl_db::simulation::*;
use std::collections::HashMap;

/// Generator for random seeds (for deterministic simulation)
pub fn seed_strategy() -> impl Strategy<Value = u64> {
    any::<u64>()
}

/// Generator for cluster sizes (3-7 nodes for quorum testing)
pub fn cluster_size_strategy() -> impl Strategy<Value = usize> {
    3usize..=7
}

/// Generator for operation sequences
#[derive(Debug, Clone)]
pub enum Operation {
    Insert { key: String, value: String },
    Update { key: String, value: String },
    Delete { key: String },
    Read { key: String },
}

pub fn operation_strategy() -> impl Strategy<Value = Operation> {
    prop_oneof![
        (any::<String>(), any::<String>()).prop_map(|(k, v)| Operation::Insert { key: k, value: v }),
        (any::<String>(), any::<String>()).prop_map(|(k, v)| Operation::Update { key: k, value: v }),
        any::<String>().prop_map(|k| Operation::Delete { key: k }),
        any::<String>().prop_map(|k| Operation::Read { key: k }),
    ]
}

/// Generator for sequences of operations
pub fn operation_sequence_strategy() -> impl Strategy<Value = Vec<Operation>> {
    prop::collection::vec(operation_strategy(), 1..20)
}

/// Generator for failure scenarios
#[derive(Debug, Clone)]
pub enum FailureScenario {
    NodeCrash { node_id: usize },
    NetworkPartition { group_a: Vec<usize>, group_b: Vec<usize> },
    DiskFailure { node_id: usize },
    SlowNetwork { delay_ms: u64 },
}

pub fn failure_scenario_strategy(max_nodes: usize) -> impl Strategy<Value = FailureScenario> {
    prop_oneof![
        (0..max_nodes).prop_map(|node_id| FailureScenario::NodeCrash { node_id }),
        (0..max_nodes).prop_map(|node_id| FailureScenario::DiskFailure { node_id }),
        (10u64..1000).prop_map(|delay_ms| FailureScenario::SlowNetwork { delay_ms }),
    ]
}

/// Generator for network partition scenarios
pub fn network_partition_strategy(num_nodes: usize) -> impl Strategy<Value = (Vec<usize>, Vec<usize>)> {
    // Create a partition where at least one node is in each group
    (1..num_nodes).prop_flat_map(move |split_point| {
        let group_a: Vec<usize> = (0..split_point).collect();
        let group_b: Vec<usize> = (split_point..num_nodes).collect();
        Just((group_a, group_b))
    })
}

/// Generator for timing scenarios (for race condition testing)
pub fn timing_scenario_strategy() -> impl Strategy<Value = Vec<u64>> {
    prop::collection::vec(0u64..1000, 2..10)
}

/// Generator for OQL-like operations
#[derive(Debug, Clone)]
pub struct OQLOperation {
    pub schema: String,
    pub operation_type: OQLOperationType,
}

#[derive(Debug, Clone)]
pub enum OQLOperationType {
    Insert { id: String, data: HashMap<String, String> },
    Update { id: String, data: HashMap<String, String> },
    Delete { id: String },
    Query { filter: String },
}

pub fn oql_operation_strategy() -> impl Strategy<Value = OQLOperation> {
    let schema_strategy = prop_oneof![
        Just("User".to_string()),
        Just("Order".to_string()),
        Just("Product".to_string()),
    ];
    
    schema_strategy.prop_flat_map(|schema| {
        let op_type_strategy = prop_oneof![
            (any::<String>(), prop::collection::hash_map(any::<String>(), any::<String>(), 1..5))
                .prop_map(|(id, data)| OQLOperationType::Insert { id, data }),
            (any::<String>(), prop::collection::hash_map(any::<String>(), any::<String>(), 1..5))
                .prop_map(|(id, data)| OQLOperationType::Update { id, data }),
            any::<String>().prop_map(|id| OQLOperationType::Delete { id }),
            any::<String>().prop_map(|filter| OQLOperationType::Query { filter }),
        ];
        
        op_type_strategy.prop_map(move |operation_type| OQLOperation {
            schema: schema.clone(),
            operation_type,
        })
    })
}

/// Generator for concurrent operation scenarios
pub fn concurrent_operations_strategy() -> impl Strategy<Value = Vec<Vec<Operation>>> {
    // Generate 2-5 concurrent threads, each with 1-10 operations
    prop::collection::vec(
        prop::collection::vec(operation_strategy(), 1..10),
        2..5
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generators_compile() {
        // Just verify generators compile and can be used
        proptest!(|(
            seed in seed_strategy(),
            cluster_size in cluster_size_strategy(),
            ops in operation_sequence_strategy(),
        )| {
            assert!(seed >= 0);
            assert!(cluster_size >= 3 && cluster_size <= 7);
            assert!(!ops.is_empty());
        });
    }
}
