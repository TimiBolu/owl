use std::sync::Arc;
use std::time::Duration;

use super::{
    Clock, FileSystem, Network, RealClock, RealFileSystem, RealNetwork, RealScheduler, Scheduler,
    SimulatedClock, SimulatedFileSystem, SimulatedNetwork, SimulatedScheduler,
};

/// Main simulation controller for deterministic testing
pub struct Simulation {
    /// Seed for deterministic behavior
    seed: u64,
    /// Simulated clock
    clock: Arc<SimulatedClock>,
    /// Simulated scheduler
    scheduler: Arc<SimulatedScheduler>,
    /// Simulated file system
    filesystem: Arc<SimulatedFileSystem>,
    /// Simulated network
    network: Arc<SimulatedNetwork>,
}

impl Simulation {
    /// Create a new simulation with the given seed
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            clock: Arc::new(SimulatedClock::new(seed)),
            scheduler: Arc::new(SimulatedScheduler::new(seed)),
            filesystem: Arc::new(SimulatedFileSystem::new(seed)),
            network: Arc::new(SimulatedNetwork::new(seed)),
        }
    }

    /// Get the simulation seed
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Get the simulated clock
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    /// Get the simulated scheduler
    pub fn scheduler(&self) -> Arc<dyn Scheduler> {
        self.scheduler.clone()
    }

    /// Get the concrete simulated scheduler for testing
    pub fn simulated_scheduler(&self) -> Arc<SimulatedScheduler> {
        self.scheduler.clone()
    }

    /// Get the simulated file system
    pub fn filesystem(&self) -> Arc<dyn FileSystem> {
        self.filesystem.clone()
    }

    /// Get the simulated network
    pub fn network(&self) -> Arc<dyn Network> {
        self.network.clone()
    }

    /// Get the concrete simulated network for testing
    pub fn simulated_network(&self) -> Arc<SimulatedNetwork> {
        self.network.clone()
    }

    /// Run a test scenario
    pub fn run<F, R>(&mut self, test: F) -> R
    where
        F: FnOnce(&mut Self) -> R,
    {
        test(self)
    }

    /// Advance simulation time by the given duration
    pub fn advance_time(&self, duration: Duration) {
        // Advance all time-dependent components
        self.clock.advance_time(duration);
        self.network.advance_time(duration);
    }

    /// Execute one step of the simulation
    pub fn step(&self) -> bool {
        // Execute one task from scheduler
        let task_executed = self.scheduler.step();

        // Process any pending clock events
        if self.clock.pending_events() > 0 {
            self.clock.advance_time(Duration::from_nanos(1));
        }

        // Advance network by small amount to deliver messages
        self.network.advance_time(Duration::from_nanos(1));

        task_executed || self.clock.pending_events() > 0
    }

    /// Run simulation until no more events are pending
    pub fn run_to_completion(&self) {
        while self.step() {
            // Continue until no more work
        }
    }

    /// Inject a crash into the file system
    pub fn crash_filesystem(&self) {
        self.filesystem.crash();
    }

    /// Recover the file system from crash
    pub fn recover_filesystem(&self) {
        self.filesystem.recover();
    }

    /// Create a network partition
    pub fn partition_network(&self, group1: &[u32], group2: &[u32]) {
        self.network.partition(group1, group2);
    }

    /// Heal network partition
    pub fn heal_network(&self) {
        self.network.heal_partition();
    }

    /// Set packet loss rate for network
    pub fn set_packet_loss(&self, rate: f64) {
        self.network.set_packet_loss_rate(rate);
    }

    /// Set failure rate for file system operations
    pub fn set_filesystem_failure_rate(&self, rate: f64) {
        self.filesystem.set_failure_rate(rate);
    }

    /// Get simulation statistics
    pub fn stats(&self) -> SimulationStats {
        SimulationStats {
            seed: self.seed,
            current_time: self.clock.now(),
            pending_events: self.clock.pending_events(),
            queued_tasks: self.scheduler.queued_tasks(),
            active_tasks: self.scheduler.active_task_count(),
        }
    }
}

/// Statistics about the current simulation state
#[derive(Debug, Clone)]
pub struct SimulationStats {
    pub seed: u64,
    pub current_time: std::time::Instant,
    pub pending_events: usize,
    pub queued_tasks: usize,
    pub active_tasks: usize,
}

/// Production environment using real implementations
pub struct ProductionEnvironment {
    clock: Arc<RealClock>,
    scheduler: Arc<RealScheduler>,
    filesystem: Arc<RealFileSystem>,
    network: Arc<RealNetwork>,
}

impl ProductionEnvironment {
    /// Create a new production environment
    pub fn new() -> Self {
        Self {
            clock: Arc::new(RealClock::new()),
            scheduler: Arc::new(RealScheduler::new()),
            filesystem: Arc::new(RealFileSystem::new()),
            network: Arc::new(RealNetwork::new()),
        }
    }

    /// Create with custom names for debugging
    pub fn with_names(
        clock_name: String,
        scheduler_name: String,
        filesystem_name: String,
        network_name: String,
    ) -> Self {
        Self {
            clock: Arc::new(RealClock::with_name(clock_name)),
            scheduler: Arc::new(RealScheduler::with_name(scheduler_name)),
            filesystem: Arc::new(RealFileSystem::with_name(filesystem_name)),
            network: Arc::new(RealNetwork::with_name(network_name)),
        }
    }

    /// Get the real clock
    pub fn clock(&self) -> Arc<dyn Clock> {
        self.clock.clone()
    }

    /// Get the real scheduler
    pub fn scheduler(&self) -> Arc<dyn Scheduler> {
        self.scheduler.clone()
    }

    /// Get the real file system
    pub fn filesystem(&self) -> Arc<dyn FileSystem> {
        self.filesystem.clone()
    }

    /// Get the real network
    pub fn network(&self) -> Arc<dyn Network> {
        self.network.clone()
    }
}

impl Default for ProductionEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

/// Helper function to create a simulation for testing
pub fn create_simulation(seed: u64) -> Simulation {
    Simulation::new(seed)
}

/// Helper function to create production environment
pub fn create_production_environment() -> ProductionEnvironment {
    ProductionEnvironment::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

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
            sim.scheduler.spawn(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }));

            sim.scheduler.run_tasks(None);
        });

        sim2.run(|sim| {
            let counter = counter2.clone();
            sim.scheduler.spawn(Box::new(move || {
                counter.fetch_add(1, Ordering::SeqCst);
            }));

            sim.scheduler.run_tasks(None);
        });

        // Results should be identical
        assert_eq!(
            counter1.load(Ordering::SeqCst),
            counter2.load(Ordering::SeqCst)
        );
    }

    #[test]
    fn test_simulation_time_control() {
        let mut sim = Simulation::new(42);

        sim.run(|sim| {
            let start_time = sim.clock().now();

            // Schedule an event
            let counter = Arc::new(AtomicU32::new(0));
            let counter_clone = counter.clone();
            sim.clock().schedule(
                Duration::from_secs(1),
                Box::new(move || {
                    counter_clone.store(42, Ordering::SeqCst);
                }),
            );

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
}
