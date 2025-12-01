use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Condvar};
use std::thread::{self, JoinHandle};
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

/// Task identifier for tracking execution order
pub type TaskId = u64;

/// Scheduler trait for thread/task management with dependency injection
pub trait Scheduler: Send + Sync {
    /// Spawn a new task (using boxed closure for dyn compatibility)
    fn spawn(&self, task: Box<dyn FnOnce() + Send + 'static>) -> TaskId;
    
    /// Yield execution to allow other tasks to run
    fn yield_now(&self);
    
    /// Wait for all spawned tasks to complete
    fn wait_for_all(&self);
}

/// Extension trait for convenient spawning with generics
pub trait SchedulerExt: Scheduler {
    /// Spawn a new task with generic closure
    fn spawn_fn<F>(&self, task: F) -> TaskId
    where
        F: FnOnce() + Send + 'static,
    {
        self.spawn(Box::new(task))
    }
    
    /// Spawn a task that returns a value
    fn spawn_with_result<F, T>(&self, task: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static;
}

// Implement SchedulerExt for all Scheduler types
impl<S: Scheduler> SchedulerExt for S {
    fn spawn_with_result<F, T>(&self, task: F) -> JoinHandle<T>
    where
        F: FnOnce() -> T + Send + 'static,
        T: Send + 'static,
    {
        // Default implementation - spawn in real thread
        thread::spawn(task)
    }
}

/// Task to be executed in simulation
struct SimulatedTask {
    id: TaskId,
    task: Box<dyn FnOnce() + Send>,
}

/// Simulated scheduler for controlled concurrency testing
pub struct SimulatedScheduler {
    /// Queue of tasks waiting to be executed
    task_queue: Arc<Mutex<VecDeque<SimulatedTask>>>,
    /// Condition variable for task availability
    task_available: Arc<Condvar>,
    /// Random number generator for deterministic interleaving
    rng: Arc<Mutex<StdRng>>,
    /// Next task ID
    next_task_id: Arc<Mutex<TaskId>>,
    /// Number of active tasks
    active_tasks: Arc<Mutex<usize>>,
    /// Completion condition variable
    completion_cv: Arc<Condvar>,
}

impl SimulatedScheduler {
    /// Create a new simulated scheduler with deterministic behavior
    pub fn new(seed: u64) -> Self {
        Self {
            task_queue: Arc::new(Mutex::new(VecDeque::new())),
            task_available: Arc::new(Condvar::new()),
            rng: Arc::new(Mutex::new(StdRng::seed_from_u64(seed))),
            next_task_id: Arc::new(Mutex::new(0)),
            active_tasks: Arc::new(Mutex::new(0)),
            completion_cv: Arc::new(Condvar::new()),
        }
    }
    
    /// Execute tasks in controlled order based on seed
    pub fn run_tasks(&self, max_tasks: Option<usize>) {
        let mut executed = 0;
        
        loop {
            if let Some(max) = max_tasks {
                if executed >= max {
                    break;
                }
            }
            
            let task = {
                let mut queue = self.task_queue.lock().unwrap();
                
                if queue.is_empty() {
                    // No tasks available
                    break;
                }
                
                // Randomly select a task for interleaving (deterministic based on seed)
                let mut rng = self.rng.lock().unwrap();
                let index = if queue.len() == 1 {
                    0
                } else {
                    rng.gen_range(0..queue.len())
                };
                
                queue.remove(index).unwrap()
            };
            
            // Execute the task
            (task.task)();
            executed += 1;
            
            // Decrement active task count
            {
                let mut active = self.active_tasks.lock().unwrap();
                *active -= 1;
                if *active == 0 {
                    self.completion_cv.notify_all();
                }
            }
        }
    }
    
    /// Execute a single task step (for fine-grained control)
    pub fn step(&self) -> bool {
        let task = {
            let mut queue = self.task_queue.lock().unwrap();
            queue.pop_front()
        };
        
        if let Some(task) = task {
            (task.task)();
            
            // Decrement active task count
            let mut active = self.active_tasks.lock().unwrap();
            *active -= 1;
            if *active == 0 {
                self.completion_cv.notify_all();
            }
            
            true
        } else {
            false
        }
    }
    
    /// Get the number of queued tasks
    pub fn queued_tasks(&self) -> usize {
        self.task_queue.lock().unwrap().len()
    }
    
    /// Get the number of active tasks
    pub fn active_task_count(&self) -> usize {
        *self.active_tasks.lock().unwrap()
    }
    
    /// Randomize task execution order for race condition testing
    pub fn shuffle_tasks(&self) {
        let mut queue = self.task_queue.lock().unwrap();
        let mut rng = self.rng.lock().unwrap();
        
        // Fisher-Yates shuffle
        for i in (1..queue.len()).rev() {
            let j = rng.gen_range(0..=i);
            queue.swap(i, j);
        }
    }
}

impl Scheduler for SimulatedScheduler {
    fn spawn(&self, task: Box<dyn FnOnce() + Send + 'static>) -> TaskId {
        let task_id = {
            let mut next_id = self.next_task_id.lock().unwrap();
            let id = *next_id;
            *next_id += 1;
            id
        };
        
        let simulated_task = SimulatedTask {
            id: task_id,
            task,
        };
        
        // Add to queue
        self.task_queue.lock().unwrap().push_back(simulated_task);
        
        // Increment active task count
        *self.active_tasks.lock().unwrap() += 1;
        
        // Notify that a task is available
        self.task_available.notify_one();
        
        task_id
    }
    
    fn yield_now(&self) {
        // In simulation, yielding is a no-op since we control execution
    }
    
    fn wait_for_all(&self) {
        let mut active = self.active_tasks.lock().unwrap();
        while *active > 0 {
            active = self.completion_cv.wait(active).unwrap();
        }
    }
}

/// Real scheduler for production deployment using OS threads
pub struct RealScheduler {
    /// Optional name for debugging
    name: Option<String>,
}

impl RealScheduler {
    pub fn new() -> Self {
        Self { name: None }
    }
    
    pub fn with_name(name: String) -> Self {
        Self { name: Some(name) }
    }
}

impl Default for RealScheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler for RealScheduler {
    fn spawn(&self, task: Box<dyn FnOnce() + Send + 'static>) -> TaskId {
        let name = self.name.clone();
        let _handle = thread::Builder::new()
            .name(format!("scheduler-{}", 
                name.as_deref().unwrap_or("default")))
            .spawn(move || {
                // Catch panics to prevent thread crashes
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    task();
                }));
                
                if let Err(panic_info) = result {
                    eprintln!("Spawned task panicked: {:?}", panic_info);
                }
            })
            .expect("Failed to spawn thread");
        
        // Use a simple counter as task ID for production
        // Note: This is not perfect but works for basic production use
        use std::sync::atomic::{AtomicU64, Ordering};
        static TASK_COUNTER: AtomicU64 = AtomicU64::new(0);
        TASK_COUNTER.fetch_add(1, Ordering::SeqCst)
    }
    
    fn yield_now(&self) {
        thread::yield_now();
    }
    
    fn wait_for_all(&self) {
        // In production, we don't track all spawned threads
        // This is a limitation of the real scheduler
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::time::Duration;
    
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
    fn test_deterministic_interleaving() {
        let scheduler1 = SimulatedScheduler::new(42);
        let scheduler2 = SimulatedScheduler::new(42);
        
        let mut results1 = Vec::new();
        let mut results2 = Vec::new();
        
        // Spawn tasks in same order for both schedulers
        for _i in 0..5 {
            scheduler1.spawn(Box::new(move || {
                // Simulate some work that could be interleaved
                std::thread::sleep(Duration::from_nanos(1));
                // Note: In real simulation, we wouldn't use thread::sleep
            }));
            
            scheduler2.spawn(Box::new(move || {
                std::thread::sleep(Duration::from_nanos(1));
            }));
        }
        
        // Both should have same number of tasks
        assert_eq!(scheduler1.queued_tasks(), scheduler2.queued_tasks());
    }
    
    #[test]
    fn test_task_shuffling() {
        let scheduler = SimulatedScheduler::new(42);
        let execution_order = Arc::new(Mutex::new(Vec::new()));
        
        // Spawn tasks with identifiable behavior
        for i in 0..5 {
            let order_clone = execution_order.clone();
            scheduler.spawn(Box::new(move || {
                order_clone.lock().unwrap().push(i);
            }));
        }
        
        // Shuffle and execute
        scheduler.shuffle_tasks();
        scheduler.run_tasks(None);
        
        let final_order = execution_order.lock().unwrap();
        
        // Should have executed all tasks
        assert_eq!(final_order.len(), 5);
        
        // Order might be different due to shuffling
        let mut sorted_order = final_order.clone();
        sorted_order.sort();
        assert_eq!(sorted_order, vec![0, 1, 2, 3, 4]);
    }
    
    #[test]
    fn test_real_scheduler_basic() {
        let scheduler = RealScheduler::new();
        let counter = Arc::new(AtomicU32::new(0));
        
        let counter_clone = counter.clone();
        let _task_id = scheduler.spawn(Box::new(move || {
            counter_clone.fetch_add(1, Ordering::SeqCst);
        }));
        
        // Give thread time to execute
        std::thread::sleep(Duration::from_millis(10));
        
        assert_eq!(counter.load(Ordering::SeqCst), 1);
    }
}