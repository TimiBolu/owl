// Thread Pool for Parallel Execution
// Work-stealing thread pool for parallel query execution

use std::sync::{Arc, Mutex, Condvar};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread;
use std::collections::VecDeque;
use crate::error::{OQLError, OQLResult};

/// A task that can be executed by the thread pool
pub type Task = Box<dyn FnOnce() + Send + 'static>;

/// Work-stealing thread pool
/// 
/// Each worker thread has its own queue. When a worker runs out of work,
/// it attempts to steal work from other workers' queues.
pub struct ThreadPool {
    workers: Vec<Worker>,
    task_queues: Arc<Vec<Mutex<VecDeque<Task>>>>,
    shutdown: Arc<AtomicBool>,
    active_tasks: Arc<AtomicUsize>,
}

impl ThreadPool {
    /// Create a new thread pool with the specified number of workers
    /// 
    /// If num_workers is 0, uses the number of logical CPUs
    pub fn new(num_workers: usize) -> OQLResult<Self> {
        let num_workers = if num_workers == 0 {
            num_cpus::get()
        } else {
            num_workers
        };
        
        if num_workers == 0 {
            return Err(OQLError::generic("Thread pool must have at least one worker"));
        }
        
        let shutdown = Arc::new(AtomicBool::new(false));
        let active_tasks = Arc::new(AtomicUsize::new(0));
        
        // Create task queues for each worker
        let mut task_queues = Vec::new();
        for _ in 0..num_workers {
            task_queues.push(Mutex::new(VecDeque::new()));
        }
        let task_queues = Arc::new(task_queues);
        
        // Create workers
        let mut workers = Vec::new();
        for id in 0..num_workers {
            workers.push(Worker::new(
                id,
                Arc::clone(&task_queues),
                Arc::clone(&shutdown),
                Arc::clone(&active_tasks),
            )?);
        }
        
        Ok(Self {
            workers,
            task_queues,
            shutdown,
            active_tasks,
        })
    }
    
    /// Execute a task on the thread pool
    /// 
    /// The task is added to a worker's queue using round-robin distribution
    pub fn execute<F>(&self, task: F) -> OQLResult<()>
    where
        F: FnOnce() + Send + 'static,
    {
        if self.shutdown.load(Ordering::SeqCst) {
            return Err(OQLError::generic("Thread pool is shutting down"));
        }
        
        // Round-robin task distribution
        let worker_id = self.active_tasks.fetch_add(1, Ordering::SeqCst) % self.workers.len();
        
        let mut queue = self.task_queues[worker_id].lock()
            .map_err(|_| OQLError::generic("Failed to lock task queue"))?;
        queue.push_back(Box::new(task));
        
        // Notify worker
        self.workers[worker_id].notify();
        
        Ok(())
    }
    
    /// Wait for all active tasks to complete
    pub fn wait_for_completion(&self) {
        while self.active_tasks.load(Ordering::SeqCst) > 0 {
            thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    
    /// Get the number of workers in the pool
    pub fn num_workers(&self) -> usize {
        self.workers.len()
    }
}

impl Drop for ThreadPool {
    fn drop(&mut self) {
        // Signal shutdown
        self.shutdown.store(true, Ordering::SeqCst);
        
        // Wake up all workers
        for worker in &self.workers {
            worker.notify();
        }
        
        // Wait for workers to finish
        for worker in self.workers.drain(..) {
            if let Some(thread) = worker.thread {
                thread.join().ok();
            }
        }
    }
}

/// A worker thread in the thread pool
struct Worker {
    id: usize,
    thread: Option<thread::JoinHandle<()>>,
    condvar: Arc<Condvar>,
}

impl Worker {
    fn new(
        id: usize,
        task_queues: Arc<Vec<Mutex<VecDeque<Task>>>>,
        shutdown: Arc<AtomicBool>,
        active_tasks: Arc<AtomicUsize>,
    ) -> OQLResult<Self> {
        let condvar = Arc::new(Condvar::new());
        let condvar_clone = Arc::clone(&condvar);
        
        let thread = thread::Builder::new()
            .name(format!("oql-worker-{}", id))
            .spawn(move || {
                Self::run(id, task_queues, shutdown, active_tasks, condvar_clone);
            })
            .map_err(|e| OQLError::generic(&format!("Failed to spawn worker thread: {}", e)))?;
        
        Ok(Self {
            id,
            thread: Some(thread),
            condvar,
        })
    }
    
    fn notify(&self) {
        self.condvar.notify_one();
    }
    
    fn run(
        id: usize,
        task_queues: Arc<Vec<Mutex<VecDeque<Task>>>>,
        shutdown: Arc<AtomicBool>,
        active_tasks: Arc<AtomicUsize>,
        condvar: Arc<Condvar>,
    ) {
        let num_workers = task_queues.len();
        
        loop {
            // Check for shutdown
            if shutdown.load(Ordering::SeqCst) {
                break;
            }
            
            // Try to get a task from own queue
            let task = {
                let mut queue = task_queues[id].lock().unwrap();
                
                // Wait if queue is empty
                while queue.is_empty() && !shutdown.load(Ordering::SeqCst) {
                    queue = condvar.wait_timeout(
                        queue,
                        std::time::Duration::from_millis(100)
                    ).unwrap().0;
                    
                    // Try work stealing
                    if queue.is_empty() {
                        drop(queue);
                        if let Some(stolen_task) = Self::steal_work(id, &task_queues, num_workers) {
                            // Execute stolen task
                            stolen_task();
                            active_tasks.fetch_sub(1, Ordering::SeqCst);
                        }
                        queue = task_queues[id].lock().unwrap();
                    }
                }
                
                queue.pop_front()
            };
            
            // Execute task if we got one
            if let Some(task) = task {
                task();
                active_tasks.fetch_sub(1, Ordering::SeqCst);
            }
        }
    }
    
    fn steal_work(
        id: usize,
        task_queues: &Arc<Vec<Mutex<VecDeque<Task>>>>,
        num_workers: usize,
    ) -> Option<Task> {
        // Try to steal from other workers in round-robin order
        for i in 1..num_workers {
            let target_id = (id + i) % num_workers;
            
            if let Ok(mut queue) = task_queues[target_id].try_lock() {
                if let Some(task) = queue.pop_back() {
                    return Some(task);
                }
            }
        }
        
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicU32;
    
    #[test]
    fn test_thread_pool_creation() {
        let pool = ThreadPool::new(4).unwrap();
        assert_eq!(pool.num_workers(), 4);
    }
    
    #[test]
    fn test_thread_pool_execute() {
        let pool = ThreadPool::new(2).unwrap();
        let counter = Arc::new(AtomicU32::new(0));
        
        for _ in 0..10 {
            let counter_clone = Arc::clone(&counter);
            pool.execute(move || {
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }).unwrap();
        }
        
        pool.wait_for_completion();
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }
    
    #[test]
    fn test_work_stealing() {
        let pool = ThreadPool::new(4).unwrap();
        let counter = Arc::new(AtomicU32::new(0));
        
        // Submit many tasks to trigger work stealing
        for _ in 0..100 {
            let counter_clone = Arc::clone(&counter);
            pool.execute(move || {
                // Simulate some work
                thread::sleep(std::time::Duration::from_micros(100));
                counter_clone.fetch_add(1, Ordering::SeqCst);
            }).unwrap();
        }
        
        pool.wait_for_completion();
        assert_eq!(counter.load(Ordering::SeqCst), 100);
    }
}
