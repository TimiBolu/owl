use crate::document::Document;
use crate::error::OQLResult;
use crate::execution::operator::{DataChunk, PhysicalOperatorExec};
use crate::storage::Storage;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Serialize)]
pub struct OperatorExecutionStats {
    pub operator: String,
    pub estimated_rows: u64,
    pub actual_rows: u64,
    pub calls: u64,
    pub elapsed_ms: f64,
    pub self_ms: f64,
    pub open_ms: f64,
    pub next_ms: f64,
    pub close_ms: f64,
    pub errors: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub metrics: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<OperatorExecutionStats>,
}

#[derive(Default)]
struct OperatorCounters {
    open_ns: AtomicU64,
    next_ns: AtomicU64,
    close_ns: AtomicU64,
    calls: AtomicU64,
    rows: AtomicU64,
    errors: AtomicU64,
}

struct ProfileNode {
    operator: String,
    estimated_rows: u64,
    parent: Option<usize>,
    counters: Arc<OperatorCounters>,
    metrics: BTreeMap<String, u64>,
}

#[derive(Default)]
struct ProfileCollector {
    nodes: Mutex<Vec<ProfileNode>>,
}

impl ProfileCollector {
    fn register(&self, operator: String, estimated_rows: u64, parent: Option<usize>) -> usize {
        let mut nodes = self.nodes.lock().expect("query profile lock poisoned");
        let id = nodes.len();
        nodes.push(ProfileNode {
            operator,
            estimated_rows,
            parent,
            counters: Arc::new(OperatorCounters::default()),
            metrics: BTreeMap::new(),
        });
        id
    }

    fn counters(&self, id: usize) -> Arc<OperatorCounters> {
        self.nodes.lock().expect("query profile lock poisoned")[id]
            .counters
            .clone()
    }

    fn set_metrics(&self, id: usize, metrics: BTreeMap<String, u64>) {
        self.nodes.lock().expect("query profile lock poisoned")[id].metrics = metrics;
    }

    fn snapshot(&self, root: usize) -> OperatorExecutionStats {
        let nodes = self.nodes.lock().expect("query profile lock poisoned");
        snapshot_node(&nodes, root)
    }
}

fn snapshot_node(nodes: &[ProfileNode], id: usize) -> OperatorExecutionStats {
    let node = &nodes[id];
    let children = nodes
        .iter()
        .enumerate()
        .filter(|(_, child)| child.parent == Some(id))
        .map(|(child_id, _)| snapshot_node(nodes, child_id))
        .collect::<Vec<_>>();
    let open_ns = node.counters.open_ns.load(Ordering::Relaxed);
    let next_ns = node.counters.next_ns.load(Ordering::Relaxed);
    let close_ns = node.counters.close_ns.load(Ordering::Relaxed);
    let elapsed_ns = open_ns.saturating_add(next_ns).saturating_add(close_ns);
    let child_ns = children
        .iter()
        .map(|child| milliseconds_to_ns(child.elapsed_ms))
        .sum::<u64>();

    OperatorExecutionStats {
        operator: node.operator.clone(),
        estimated_rows: node.estimated_rows,
        actual_rows: node.counters.rows.load(Ordering::Relaxed),
        calls: node.counters.calls.load(Ordering::Relaxed),
        elapsed_ms: ns_to_milliseconds(elapsed_ns),
        self_ms: ns_to_milliseconds(elapsed_ns.saturating_sub(child_ns)),
        open_ms: ns_to_milliseconds(open_ns),
        next_ms: ns_to_milliseconds(next_ns),
        close_ms: ns_to_milliseconds(close_ns),
        errors: node.counters.errors.load(Ordering::Relaxed),
        metrics: node.metrics.clone(),
        children,
    }
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn ns_to_milliseconds(ns: u64) -> f64 {
    Duration::from_nanos(ns).as_secs_f64() * 1_000.0
}

fn milliseconds_to_ns(milliseconds: f64) -> u64 {
    (milliseconds * 1_000_000.0)
        .round()
        .clamp(0.0, u64::MAX as f64) as u64
}

struct ProfiledExec {
    inner: Box<dyn PhysicalOperatorExec>,
    collector: Arc<ProfileCollector>,
    id: usize,
    counters: Arc<OperatorCounters>,
}

impl ProfiledExec {
    fn new(
        inner: Box<dyn PhysicalOperatorExec>,
        collector: Arc<ProfileCollector>,
        id: usize,
    ) -> Self {
        let counters = collector.counters(id);
        Self {
            inner,
            collector,
            id,
            counters,
        }
    }

    fn record_result<T>(&self, result: &OQLResult<T>) {
        if result.is_err() {
            self.counters.errors.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl PhysicalOperatorExec for ProfiledExec {
    fn open(&mut self, storage: Storage) -> OQLResult<()> {
        let started = Instant::now();
        let result = self.inner.open(storage);
        self.counters
            .open_ns
            .fetch_add(elapsed_ns(started), Ordering::Relaxed);
        self.record_result(&result);
        result
    }

    fn next(&mut self) -> OQLResult<Option<Document>> {
        let started = Instant::now();
        let result = self.inner.next();
        self.counters
            .next_ns
            .fetch_add(elapsed_ns(started), Ordering::Relaxed);
        self.counters.calls.fetch_add(1, Ordering::Relaxed);
        if matches!(result, Ok(Some(_))) {
            self.counters.rows.fetch_add(1, Ordering::Relaxed);
        }
        self.record_result(&result);
        result
    }

    fn next_batch(&mut self, target_rows: usize) -> OQLResult<Option<DataChunk>> {
        let started = Instant::now();
        let result = self.inner.next_batch(target_rows);
        self.counters
            .next_ns
            .fetch_add(elapsed_ns(started), Ordering::Relaxed);
        self.counters.calls.fetch_add(1, Ordering::Relaxed);
        if let Ok(Some(chunk)) = &result {
            self.counters
                .rows
                .fetch_add(chunk.len() as u64, Ordering::Relaxed);
        }
        self.record_result(&result);
        result
    }

    fn close(&mut self) -> OQLResult<()> {
        let started = Instant::now();
        let result = self.inner.close();
        self.counters
            .close_ns
            .fetch_add(elapsed_ns(started), Ordering::Relaxed);
        self.collector
            .set_metrics(self.id, self.inner.runtime_metrics());
        self.record_result(&result);
        result
    }

    fn reset(&mut self) -> OQLResult<()> {
        self.inner.reset()
    }
}

#[derive(Clone)]
pub struct ExecutionProfile {
    collector: Arc<ProfileCollector>,
    root: usize,
}

impl ExecutionProfile {
    pub fn snapshot(&self) -> OperatorExecutionStats {
        self.collector.snapshot(self.root)
    }
}

pub(crate) struct ProfileBuilder {
    collector: Arc<ProfileCollector>,
}

impl ProfileBuilder {
    pub(crate) fn new() -> Self {
        Self {
            collector: Arc::new(ProfileCollector::default()),
        }
    }

    pub(crate) fn register(
        &self,
        operator: String,
        estimated_rows: u64,
        parent: Option<usize>,
    ) -> usize {
        self.collector.register(operator, estimated_rows, parent)
    }

    pub(crate) fn wrap(
        &self,
        id: usize,
        inner: Box<dyn PhysicalOperatorExec>,
    ) -> Box<dyn PhysicalOperatorExec> {
        Box::new(ProfiledExec::new(inner, self.collector.clone(), id))
    }

    pub(crate) fn finish(&self, root: usize) -> ExecutionProfile {
        ExecutionProfile {
            collector: self.collector.clone(),
            root,
        }
    }
}
