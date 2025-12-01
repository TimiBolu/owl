// Simulation Framework for Deterministic Testing
//
// This module provides the core simulation infrastructure for testing distributed
// systems behavior in a controlled, deterministic environment.

pub mod clock;
pub mod scheduler;
pub mod filesystem;
pub mod network;
pub mod controller;

pub use clock::{Clock, SimulatedClock, RealClock};
pub use scheduler::{Scheduler, SimulatedScheduler, RealScheduler};
pub use filesystem::{FileSystem, SimulatedFileSystem, RealFileSystem};
pub use network::{Network, SimulatedNetwork, RealNetwork};
pub use controller::Simulation;

// Re-export common types
pub use std::time::{Duration, Instant};
pub use std::sync::Arc;