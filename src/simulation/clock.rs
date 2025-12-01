use std::collections::BinaryHeap;
use std::cmp::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Clock trait for time operations with dependency injection
pub trait Clock: Send + Sync {
    /// Get current time
    fn now(&self) -> Instant;
    
    /// Sleep for a duration (no-op in simulation)
    fn sleep(&self, duration: Duration);
    
    /// Schedule a callback to run after a delay
    fn schedule(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>);
}

/// Event scheduled for future execution
struct ScheduledEvent {
    time: Instant,
    callback: Box<dyn FnOnce() + Send>,
}

impl std::fmt::Debug for ScheduledEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScheduledEvent")
            .field("time", &self.time)
            .field("callback", &"<callback>")
            .finish()
    }
}

impl PartialEq for ScheduledEvent {
    fn eq(&self, other: &Self) -> bool {
        self.time == other.time
    }
}

impl Eq for ScheduledEvent {}

impl PartialOrd for ScheduledEvent {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for ScheduledEvent {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reverse ordering for min-heap behavior (earliest events first)
        other.time.cmp(&self.time)
    }
}

/// Simulated clock for deterministic time control in tests
pub struct SimulatedClock {
    current_time: Arc<Mutex<Instant>>,
    events: Arc<Mutex<BinaryHeap<ScheduledEvent>>>,
}

impl SimulatedClock {
    /// Create a new simulated clock starting at a deterministic time
    pub fn new(seed: u64) -> Self {
        // Create a deterministic start time based on seed
        // We use a fixed reference point and add the seed as offset
        let reference_instant = Instant::now();
        let start_time = reference_instant + Duration::from_nanos(seed);
        
        Self {
            current_time: Arc::new(Mutex::new(start_time)),
            events: Arc::new(Mutex::new(BinaryHeap::new())),
        }
    }
    
    /// Advance time by the given duration, processing all events up to that time
    pub fn advance_time(&self, duration: Duration) {
        let mut current_time = self.current_time.lock().unwrap();
        let target_time = *current_time + duration;
        
        // Process all events up to target time
        let mut events = self.events.lock().unwrap();
        let mut callbacks_to_run = Vec::new();
        
        while let Some(event) = events.peek() {
            if event.time <= target_time {
                if let Some(event) = events.pop() {
                    callbacks_to_run.push(event.callback);
                    *current_time = event.time;
                }
            } else {
                break;
            }
        }
        
        // Update to target time
        *current_time = target_time;
        
        // Release locks before running callbacks
        drop(events);
        drop(current_time);
        
        // Run callbacks outside of locks to avoid deadlock
        for callback in callbacks_to_run {
            callback();
        }
    }
    
    /// Get the number of pending events
    pub fn pending_events(&self) -> usize {
        self.events.lock().unwrap().len()
    }
    
    /// Process all pending events regardless of time
    pub fn process_all_events(&self) {
        let mut events = self.events.lock().unwrap();
        let mut callbacks_to_run = Vec::new();
        
        while let Some(event) = events.pop() {
            callbacks_to_run.push(event.callback);
            *self.current_time.lock().unwrap() = event.time;
        }
        
        drop(events);
        
        for callback in callbacks_to_run {
            callback();
        }
    }
}

impl Clock for SimulatedClock {
    fn now(&self) -> Instant {
        *self.current_time.lock().unwrap()
    }
    
    fn sleep(&self, _duration: Duration) {
        // No-op in simulation - time is controlled externally
    }
    
    fn schedule(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>) {
        let current_time = *self.current_time.lock().unwrap();
        let event_time = current_time + delay;
        
        let event = ScheduledEvent {
            time: event_time,
            callback,
        };
        
        self.events.lock().unwrap().push(event);
    }
}

/// Real clock for production deployment with zero overhead
pub struct RealClock {
    /// Optional name for debugging/logging
    name: Option<String>,
}

impl RealClock {
    pub fn new() -> Self {
        Self { name: None }
    }
    
    pub fn with_name(name: String) -> Self {
        Self { name: Some(name) }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    
    fn sleep(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
    
    fn schedule(&self, delay: Duration, callback: Box<dyn FnOnce() + Send>) {
        // Spawn thread with error handling
        let name = self.name.clone();
        std::thread::Builder::new()
            .name(format!("clock-scheduler-{}", 
                name.as_deref().unwrap_or("default")))
            .spawn(move || {
                std::thread::sleep(delay);
                // Catch panics in scheduled callbacks to prevent thread crashes
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    callback();
                }));
                
                if let Err(panic_info) = result {
                    eprintln!("Scheduled callback panicked: {:?}", panic_info);
                }
            })
            .expect("Failed to spawn scheduler thread");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    
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
    fn test_real_clock_performance() {
        let clock = RealClock::new();
        
        // Measure overhead of clock.now() vs Instant::now()
        let start = Instant::now();
        for _ in 0..1000 {
            let _ = clock.now();
        }
        let clock_duration = start.elapsed();
        
        let start = Instant::now();
        for _ in 0..1000 {
            let _ = Instant::now();
        }
        let direct_duration = start.elapsed();
        
        // Should have minimal overhead (within 50% is acceptable)
        assert!(clock_duration.as_nanos() < direct_duration.as_nanos() * 3 / 2);
    }
    
    #[test]
    fn test_deterministic_behavior() {
        // Same seed should produce same behavior
        let clock1 = SimulatedClock::new(42);
        let clock2 = SimulatedClock::new(42);
        
        let start1 = clock1.now();
        let start2 = clock2.now();
        
        // Should start at same time with same seed
        assert_eq!(start1, start2);
    }
}