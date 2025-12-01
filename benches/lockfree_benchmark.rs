// Benchmark comparing lock-free vs mutex-based concurrency implementations
//
// Tests performance improvement of lock-free transaction map over mutex-based registry
//
// Requirements: 12

use criterion::{black_box, criterion_group, criterion_main, Criterion, BenchmarkId};
use owl_db::concurrency::ConcurrencyManager;
use owl_db::lockfree_concurrency::{LockFreeConcurrencyManager, LockFreeTransactionMap};
use owl_db::simulation::RealClock;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn benchmark_transaction_registration(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_registration");
    
    // Test different numbers of transactions
    for num_transactions in [100, 1000, 10000].iter() {
        // Mutex-based implementation
        group.bench_with_input(
            BenchmarkId::new("mutex_based", num_transactions),
            num_transactions,
            |b, &num_transactions| {
                b.iter(|| {
                    let clock = Arc::new(RealClock::new());
                    let manager = ConcurrencyManager::new(clock);
                    
                    for i in 0..num_transactions {
                        let txn_id = format!("txn_{}", i);
                        black_box(manager.register_transaction(&txn_id).unwrap());
                    }
                    
                    for i in 0..num_transactions {
                        let txn_id = format!("txn_{}", i);
                        black_box(manager.unregister_transaction(&txn_id).unwrap());
                    }
                });
            },
        );
        
        // Lock-free implementation
        group.bench_with_input(
            BenchmarkId::new("lock_free", num_transactions),
            num_transactions,
            |b, &num_transactions| {
                b.iter(|| {
                    let clock = Arc::new(RealClock::new());
                    let manager = LockFreeConcurrencyManager::new(clock);
                    
                    for i in 0..num_transactions {
                        let txn_id = format!("txn_{}", i);
                        black_box(manager.register_transaction(&txn_id).unwrap());
                    }
                    
                    for i in 0..num_transactions {
                        let txn_id = format!("txn_{}", i);
                        black_box(manager.unregister_transaction(&txn_id).unwrap());
                    }
                });
            },
        );
    }
    
    group.finish();
}

fn benchmark_concurrent_registration(c: &mut Criterion) {
    let mut group = c.benchmark_group("concurrent_registration");
    
    // Test different numbers of threads
    for num_threads in [2, 4, 8, 16].iter() {
        // Mutex-based implementation
        group.bench_with_input(
            BenchmarkId::new("mutex_based", num_threads),
            num_threads,
            |b, &num_threads| {
                b.iter(|| {
                    let clock = Arc::new(RealClock::new());
                    let manager = Arc::new(ConcurrencyManager::new(clock));
                    
                    let mut handles = Vec::new();
                    
                    for thread_id in 0..num_threads {
                        let manager_clone = manager.clone();
                        let handle = thread::spawn(move || {
                            for i in 0..100 {
                                let txn_id = format!("txn_{}_{}", thread_id, i);
                                black_box(manager_clone.register_transaction(&txn_id).unwrap());
                                black_box(manager_clone.unregister_transaction(&txn_id).unwrap());
                            }
                        });
                        handles.push(handle);
                    }
                    
                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
        
        // Lock-free implementation
        group.bench_with_input(
            BenchmarkId::new("lock_free", num_threads),
            num_threads,
            |b, &num_threads| {
                b.iter(|| {
                    let clock = Arc::new(RealClock::new());
                    let manager = Arc::new(LockFreeConcurrencyManager::new(clock));
                    
                    let mut handles = Vec::new();
                    
                    for thread_id in 0..num_threads {
                        let manager_clone = manager.clone();
                        let handle = thread::spawn(move || {
                            for i in 0..100 {
                                let txn_id = format!("txn_{}_{}", thread_id, i);
                                black_box(manager_clone.register_transaction(&txn_id).unwrap());
                                black_box(manager_clone.unregister_transaction(&txn_id).unwrap());
                            }
                        });
                        handles.push(handle);
                    }
                    
                    for handle in handles {
                        handle.join().unwrap();
                    }
                });
            },
        );
    }
    
    group.finish();
}

fn benchmark_transaction_lookup(c: &mut Criterion) {
    let mut group = c.benchmark_group("transaction_lookup");
    
    // Pre-populate with transactions
    let num_transactions = 1000;
    
    // Mutex-based implementation
    group.bench_function("mutex_based_lookup", |b| {
        let clock = Arc::new(RealClock::new());
        let manager = ConcurrencyManager::new(clock);
        
        // Pre-populate
        for i in 0..num_transactions {
            let txn_id = format!("txn_{}", i);
            manager.register_transaction(&txn_id).unwrap();
        }
        
        b.iter(|| {
            for i in 0..100 {
                let txn_id = format!("txn_{}", i % num_transactions);
                // Simulate lookup by checking statistics
                black_box(manager.get_statistics().unwrap());
            }
        });
    });
    
    // Lock-free implementation
    group.bench_function("lock_free_lookup", |b| {
        let clock = Arc::new(RealClock::new());
        let map = LockFreeTransactionMap::new(clock);
        
        // Pre-populate
        for i in 0..num_transactions {
            let txn_id = format!("txn_{}", i);
            map.register_transaction(&txn_id).unwrap();
        }
        
        b.iter(|| {
            for i in 0..100 {
                let txn_id = format!("txn_{}", i % num_transactions);
                black_box(map.contains_transaction(&txn_id));
            }
        });
    });
    
    group.finish();
}

fn benchmark_mixed_workload(c: &mut Criterion) {
    let mut group = c.benchmark_group("mixed_workload");
    
    // Simulate a mixed workload with registrations, lookups, and unregistrations
    group.bench_function("mutex_based_mixed", |b| {
        b.iter(|| {
            let clock = Arc::new(RealClock::new());
            let manager = Arc::new(ConcurrencyManager::new(clock));
            
            let mut handles = Vec::new();
            
            // Thread 1: Continuous registration/unregistration
            let manager1 = manager.clone();
            let handle1 = thread::spawn(move || {
                for i in 0..200 {
                    let txn_id = format!("reg_txn_{}", i);
                    black_box(manager1.register_transaction(&txn_id).unwrap());
                    if i % 10 == 0 {
                        thread::sleep(Duration::from_nanos(1)); // Tiny delay
                    }
                    black_box(manager1.unregister_transaction(&txn_id).unwrap());
                }
            });
            handles.push(handle1);
            
            // Thread 2: Lookups via statistics
            let manager2 = manager.clone();
            let handle2 = thread::spawn(move || {
                for _ in 0..100 {
                    black_box(manager2.get_statistics().unwrap());
                    thread::sleep(Duration::from_nanos(1)); // Tiny delay
                }
            });
            handles.push(handle2);
            
            for handle in handles {
                handle.join().unwrap();
            }
        });
    });
    
    group.bench_function("lock_free_mixed", |b| {
        b.iter(|| {
            let clock = Arc::new(RealClock::new());
            let manager = Arc::new(LockFreeConcurrencyManager::new(clock));
            
            let mut handles = Vec::new();
            
            // Thread 1: Continuous registration/unregistration
            let manager1 = manager.clone();
            let handle1 = thread::spawn(move || {
                for i in 0..200 {
                    let txn_id = format!("reg_txn_{}", i);
                    black_box(manager1.register_transaction(&txn_id).unwrap());
                    if i % 10 == 0 {
                        thread::sleep(Duration::from_nanos(1)); // Tiny delay
                    }
                    black_box(manager1.unregister_transaction(&txn_id).unwrap());
                }
            });
            handles.push(handle1);
            
            // Thread 2: Lookups via statistics
            let manager2 = manager.clone();
            let handle2 = thread::spawn(move || {
                for _ in 0..100 {
                    black_box(manager2.get_statistics());
                    thread::sleep(Duration::from_nanos(1)); // Tiny delay
                }
            });
            handles.push(handle2);
            
            for handle in handles {
                handle.join().unwrap();
            }
        });
    });
    
    group.finish();
}

fn benchmark_memory_usage(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_usage");
    
    // Test memory allocation patterns
    group.bench_function("mutex_based_memory", |b| {
        b.iter(|| {
            let clock = Arc::new(RealClock::new());
            let manager = ConcurrencyManager::new(clock);
            
            // Allocate many transactions
            for i in 0..1000 {
                let txn_id = format!("mem_txn_{}", i);
                black_box(manager.register_transaction(&txn_id).unwrap());
            }
            
            // Clean up
            for i in 0..1000 {
                let txn_id = format!("mem_txn_{}", i);
                black_box(manager.unregister_transaction(&txn_id).unwrap());
            }
        });
    });
    
    group.bench_function("lock_free_memory", |b| {
        b.iter(|| {
            let clock = Arc::new(RealClock::new());
            let manager = LockFreeConcurrencyManager::new(clock);
            
            // Allocate many transactions
            for i in 0..1000 {
                let txn_id = format!("mem_txn_{}", i);
                black_box(manager.register_transaction(&txn_id).unwrap());
            }
            
            // Clean up
            for i in 0..1000 {
                let txn_id = format!("mem_txn_{}", i);
                black_box(manager.unregister_transaction(&txn_id).unwrap());
            }
            
            // Trigger cleanup
            manager.cleanup();
        });
    });
    
    group.finish();
}

criterion_group!(
    benches,
    benchmark_transaction_registration,
    benchmark_concurrent_registration,
    benchmark_transaction_lookup,
    benchmark_mixed_workload,
    benchmark_memory_usage
);

criterion_main!(benches);