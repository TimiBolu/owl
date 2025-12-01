// Test for dependency injection in simulation framework

use owl_db::simulation::{Clock, SimulatedClock, RealClock, Simulation};
use owl_db::executor::QueryExecutor;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn test_clock_dependency_injection() {
    // Test that SimulatedClock can be injected
    let sim_clock = Arc::new(SimulatedClock::new(42));
    let start_time = sim_clock.now();
    
    // Advance time in simulation
    sim_clock.advance_time(Duration::from_secs(1));
    let end_time = sim_clock.now();
    
    // Time should have advanced by exactly 1 second
    assert_eq!(end_time.duration_since(start_time), Duration::from_secs(1));
}

#[test]
fn test_real_clock_dependency_injection() {
    // Test that RealClock works normally
    let real_clock = Arc::new(RealClock::new());
    let start_time = real_clock.now();
    
    // Sleep for a small amount
    std::thread::sleep(Duration::from_millis(1));
    let end_time = real_clock.now();
    
    // Time should have advanced
    assert!(end_time > start_time);
}

#[test]
fn test_query_executor_with_simulated_clock() {
    use owl_db::storage::zig_adapter::ZigStore;
    
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path().join("test.owl");
    let store = Arc::new(ZigStore::new(path.to_str().unwrap(), 10 * 1024 * 1024).unwrap());
    
    // Create executor with simulated clock
    let sim_clock = Arc::new(SimulatedClock::new(42));
    let executor = QueryExecutor::new(store, sim_clock.clone());
    
    // Verify the executor was created successfully
    // The function registry includes built-in functions, so we just check it's not empty
    assert!(executor.function_names().len() > 0); // Built-in functions are registered
}

#[test]
fn test_simulation_framework_integration() {
    // Test that the full simulation framework works
    let mut sim = Simulation::new(42);
    
    // Test that we can get simulation components
    let clock = sim.clock();
    let _scheduler = sim.scheduler();
    let _filesystem = sim.filesystem();
    
    // Verify they're working
    let start_time = clock.now();
    sim.advance_time(Duration::from_millis(100));
    let end_time = clock.now();
    
    assert_eq!(end_time.duration_since(start_time), Duration::from_millis(100));
}

#[test]
fn test_deterministic_behavior() {
    // Test that same seed produces same relative behavior
    let sim1 = Simulation::new(42);
    let sim2 = Simulation::new(42);
    
    let start1 = sim1.clock().now();
    let start2 = sim2.clock().now();
    
    // Advance both by the same amount
    sim1.advance_time(Duration::from_secs(1));
    sim2.advance_time(Duration::from_secs(1));
    
    let end1 = sim1.clock().now();
    let end2 = sim2.clock().now();
    
    // The duration should be the same even if start times differ
    assert_eq!(end1.duration_since(start1), end2.duration_since(start2));
    assert_eq!(end1.duration_since(start1), Duration::from_secs(1));
}