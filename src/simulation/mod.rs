// Simulation Framework for Deterministic Testing
//
// This module provides the core simulation infrastructure for testing distributed
// systems behavior in a controlled, deterministic environment.

pub mod clock;
pub mod controller;
pub mod filesystem;
pub mod network;
pub mod scheduler;

pub use clock::{Clock, RealClock, SimulatedClock};
pub use controller::Simulation;
pub use filesystem::{FileSystem, RealFileSystem, SimulatedFileSystem};
pub use network::{Network, RealNetwork, SimulatedNetwork};
pub use scheduler::{RealScheduler, Scheduler, SimulatedScheduler};

// Re-export common types
pub use std::sync::Arc;
pub use std::time::{Duration, Instant};
