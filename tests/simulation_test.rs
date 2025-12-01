use owl_db::simulation::*;
use owl_db::ProductionEnvironment;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn test_simulated_clock_basic() {
    let clock = SimulatedClock::new(42);
    let start_time = clock.now();
    
    // Time doesn't advance automatically
    std::thread::sleep(Duration::from_millis(1));
    assert_eq!(clock.now(), start_time);
    
    // Advance time manually
    clock.advance_time(Duration::from_secs(1));
    assert_eq!(clock.now(), start_time + Duration::from_secs(1));
}

#[test]
fn test_scheduled_events_ordering() {
    let clock = SimulatedClock::new(42);
    let counter = Arc::new(AtomicU32::new(0));
    
    // Schedule events in reverse order
    let counter_clone = counter.clone();
    clock.schedule(Duration::from_secs(3), Box::new(move || {
        counter_clone.store(3, Ordering::SeqCst);
    }));
    
    let counter_clone = counter.clone();
    clock.schedule(Duration::from_secs(1), Box::new(move || {
        counter_clone.store(1, Ordering::SeqCst);
    }));
    
    let counter_clone = counter.clone();
    clock.schedule(Duration::from_secs(2), Box::new(move || {
        counter_clone.store(2, Ordering::SeqCst);
    }));
    
    // Events should fire in chronological order
    clock.advance_time(Duration::from_secs(1));
    assert_eq!(counter.load(Ordering::SeqCst), 1);
    
    clock.advance_time(Duration::from_secs(1));
    assert_eq!(counter.load(Ordering::SeqCst), 2);
    
    clock.advance_time(Duration::from_secs(1));
    assert_eq!(counter.load(Ordering::SeqCst), 3);
}

#[test]
fn test_simulated_scheduler_basic() {
    let scheduler = SimulatedScheduler::new(42);
    let counter = Arc::new(AtomicU32::new(0));
    
    // Spawn some tasks
    let counter_clone = counter.clone();
    scheduler.spawn(Box::new(move || {
        counter_clone.fetch_add(1, Ordering::SeqCst);
    }));
    
    let counter_clone = counter.clone();
    scheduler.spawn(Box::new(move || {
        counter_clone.fetch_add(2, Ordering::SeqCst);
    }));
    
    // Tasks are queued but not executed yet
    assert_eq!(counter.load(Ordering::SeqCst), 0);
    assert_eq!(scheduler.queued_tasks(), 2);
    
    // Execute tasks
    scheduler.run_tasks(None);
    
    // All tasks should be executed
    assert_eq!(counter.load(Ordering::SeqCst), 3);
    assert_eq!(scheduler.queued_tasks(), 0);
}

#[test]
fn test_simulated_filesystem_basic() {
    let fs = SimulatedFileSystem::new(42);
    let path = std::path::Path::new("test.txt");
    
    // Create and write to file
    let mut file = fs.create(path).unwrap();
    file.write(b"Hello, World!").unwrap();
    
    // File should exist
    assert!(fs.exists(path));
    assert_eq!(fs.file_size(path).unwrap(), 13);
    
    // Read from file
    let mut file = fs.open(path).unwrap();
    let mut buf = vec![0u8; 13];
    let bytes_read = file.read(&mut buf).unwrap();
    assert_eq!(bytes_read, 13);
    assert_eq!(&buf, b"Hello, World!");
}

#[test]
fn test_crash_recovery() {
    let fs = SimulatedFileSystem::new(42);
    let path = std::path::Path::new("test.txt");
    
    // Write data without sync
    let mut file = fs.create(path).unwrap();
    file.write(b"Unsynced data").unwrap();
    
    // Crash the system
    fs.crash();
    
    // Data should be lost
    assert!(!fs.exists(path));
    
    // Recover and try again with sync
    fs.recover();
    let mut file = fs.create(path).unwrap();
    file.write(b"Synced data").unwrap();
    file.sync().unwrap();
    
    // Crash again
    fs.crash();
    
    // Synced data should survive
    assert!(fs.exists(path));
    
    fs.recover();
    let mut file = fs.open(path).unwrap();
    let mut buf = vec![0u8; 11];
    file.read(&mut buf).unwrap();
    assert_eq!(&buf, b"Synced data");
}

#[test]
fn test_simulation_basic() {
    let sim = Simulation::new(42);
    
    let stats = sim.stats();
    assert_eq!(stats.seed, 42);
    assert_eq!(stats.pending_events, 0);
    assert_eq!(stats.queued_tasks, 0);
}

#[test]
fn test_deterministic_behavior() {
    let mut sim1 = Simulation::new(42);
    let mut sim2 = Simulation::new(42);
    
    let counter1 = Arc::new(AtomicU32::new(0));
    let counter2 = Arc::new(AtomicU32::new(0));
    
    // Run identical scenarios
    sim1.run(|sim| {
        let counter = counter1.clone();
        sim.scheduler().spawn(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        
        sim.simulated_scheduler().run_tasks(None);
    });
    
    sim2.run(|sim| {
        let counter = counter2.clone();
        sim.scheduler().spawn(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
        }));
        
        sim.simulated_scheduler().run_tasks(None);
    });
    
    // Results should be identical
    assert_eq!(counter1.load(Ordering::SeqCst), counter2.load(Ordering::SeqCst));
}

#[test]
fn test_simulation_time_control() {
    let mut sim = Simulation::new(42);
    
    sim.run(|sim| {
        let start_time = sim.clock().now();
        
        // Schedule an event
        let counter = Arc::new(AtomicU32::new(0));
        let counter_clone = counter.clone();
        sim.clock().schedule(Duration::from_secs(1), Box::new(move || {
            counter_clone.store(42, Ordering::SeqCst);
        }));
        
        // Event shouldn't fire yet
        assert_eq!(counter.load(Ordering::SeqCst), 0);
        
        // Advance time
        sim.advance_time(Duration::from_secs(1));
        
        // Event should have fired
        assert_eq!(counter.load(Ordering::SeqCst), 42);
        
        // Time should have advanced
        assert_eq!(sim.clock().now(), start_time + Duration::from_secs(1));
    });
}

#[test]
fn test_failure_injection() {
    let mut sim = Simulation::new(42);
    
    sim.run(|sim| {
        // Inject filesystem crash
        sim.crash_filesystem();
        
        // File operations should fail
        let path = std::path::Path::new("test.txt");
        let result = sim.filesystem().create(path);
        assert!(result.is_err());
        
        // Recover filesystem
        sim.recover_filesystem();
        
        // Operations should work again
        let result = sim.filesystem().create(path);
        assert!(result.is_ok());
    });
}

#[test]
fn test_production_environment() {
    let env = ProductionEnvironment::new();
    
    // Should be able to get all components
    let _clock = env.clock();
    let _scheduler = env.scheduler();
    let _filesystem = env.filesystem();
    let _network = env.network();
}