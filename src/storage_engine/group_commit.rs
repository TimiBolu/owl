use super::{RustStore, WriteOperation};
use crate::document::Document;
use crate::error::{OQLError, OQLResult};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

enum Message {
    Commit(Request),
    Shutdown,
}

struct Request {
    documents: Vec<Document>,
    reply: Sender<Result<u64, String>>,
}

/// Opt-in synchronous group commit for concurrent ingest producers.
///
/// Requests arriving within `max_delay` share one atomic RustStore WAL batch
/// and durability sync. If a combined cohort fails validation, requests are
/// retried separately so one logical conflict does not reject unrelated work.
pub struct GroupCommitter {
    sender: Sender<Message>,
    worker: Option<JoinHandle<()>>,
    store: Arc<RustStore>,
}

impl GroupCommitter {
    pub fn new(
        store: Arc<RustStore>,
        max_documents: usize,
        max_delay: Duration,
    ) -> OQLResult<Self> {
        let (sender, receiver) = mpsc::channel();
        let worker_store = store.clone();
        let worker = thread::Builder::new()
            .name("owl-group-commit".to_string())
            .spawn(move || run_worker(worker_store, receiver, max_documents.max(1), max_delay))
            .map_err(|error| {
                OQLError::generic(&format!("cannot start group commit worker: {error}"))
            })?;
        Ok(Self {
            sender,
            worker: Some(worker),
            store,
        })
    }

    pub fn commit_documents(&self, documents: Vec<Document>) -> OQLResult<u64> {
        if documents.is_empty() {
            return Ok(self.store.committed_lsn());
        }
        let (reply, response) = mpsc::channel();
        self.sender
            .send(Message::Commit(Request { documents, reply }))
            .map_err(|_| OQLError::generic("group commit worker is unavailable"))?;
        match response.recv() {
            Ok(Ok(lsn)) => Ok(lsn),
            Ok(Err(message)) => Err(OQLError::generic(&message)),
            Err(_) => Err(OQLError::generic(
                "group commit worker stopped before acknowledging the request",
            )),
        }
    }
}

impl Drop for GroupCommitter {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_worker(
    store: Arc<RustStore>,
    receiver: Receiver<Message>,
    max_documents: usize,
    max_delay: Duration,
) {
    let mut carry = None;
    loop {
        let first = match carry
            .take()
            .map(Message::Commit)
            .or_else(|| receiver.recv().ok())
        {
            Some(Message::Commit(request)) => request,
            Some(Message::Shutdown) | None => break,
        };
        let mut cohort = vec![first];
        let mut document_count = cohort[0].documents.len();
        let deadline = Instant::now() + max_delay;
        let mut shutdown = false;

        while document_count < max_documents {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                break;
            };
            match receiver.recv_timeout(remaining) {
                Ok(Message::Commit(request)) => {
                    if document_count > 0
                        && document_count.saturating_add(request.documents.len()) > max_documents
                    {
                        carry = Some(request);
                        break;
                    }
                    document_count = document_count.saturating_add(request.documents.len());
                    cohort.push(request);
                }
                Ok(Message::Shutdown) => {
                    shutdown = true;
                    break;
                }
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    shutdown = true;
                    break;
                }
            }
        }

        commit_cohort(&store, cohort);
        if shutdown {
            break;
        }
    }
}

fn commit_cohort(store: &RustStore, cohort: Vec<Request>) {
    let documents = cohort
        .iter()
        .flat_map(|request| request.documents.iter().cloned())
        .collect::<Vec<_>>();
    match commit_direct(store, &documents) {
        Ok(lsn) => {
            for request in cohort {
                let _ = request.reply.send(Ok(lsn));
            }
        }
        Err(_) if cohort.len() > 1 => {
            // Validation failures append no WAL frame. Retrying each request
            // separately isolates conflicting producers while retaining one
            // sync for the normal successful cohort path.
            for request in cohort {
                let result =
                    commit_direct(store, &request.documents).map_err(|error| error.to_string());
                let _ = request.reply.send(result);
            }
        }
        Err(error) => {
            let message = error.to_string();
            for request in cohort {
                let _ = request.reply.send(Err(message.clone()));
            }
        }
    }
}

fn commit_direct(store: &RustStore, documents: &[Document]) -> OQLResult<u64> {
    store.write_batch(
        documents
            .iter()
            .cloned()
            .map(WriteOperation::PutDocument)
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Value;
    use crate::storage::DocumentStore;
    use std::collections::{HashMap, HashSet};
    use std::sync::{Arc, Barrier};
    use tempfile::TempDir;

    #[test]
    fn concurrent_requests_share_one_durable_lsn() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("group-commit.owl");
        let store = Arc::new(RustStore::new(&path, 10 * 1024 * 1024).unwrap());
        let committer =
            Arc::new(GroupCommitter::new(store.clone(), 64, Duration::from_millis(100)).unwrap());
        let barrier = Arc::new(Barrier::new(9));
        let before = store.committed_lsn();
        let mut workers = Vec::new();
        for number in 0..8 {
            let committer = committer.clone();
            let barrier = barrier.clone();
            workers.push(thread::spawn(move || {
                barrier.wait();
                committer
                    .commit_documents(vec![Document::with_id(
                        number.to_string(),
                        "events".to_string(),
                        HashMap::from([("sequence".to_string(), Value::Integer(number))]),
                    )])
                    .unwrap()
            }));
        }
        barrier.wait();
        let lsns = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<HashSet<_>>();
        assert_eq!(lsns, HashSet::from([before + 1]));
        assert_eq!(store.committed_lsn(), before + 1);
        assert_eq!(store.scan_documents("events").unwrap().len(), 8);
        drop(committer);
        drop(store);
        let reopened = RustStore::new(&path, 10 * 1024 * 1024).unwrap();
        assert_eq!(reopened.scan_documents("events").unwrap().len(), 8);
    }

    #[test]
    fn conflicting_cohort_retries_requests_individually() {
        use crate::storage::ScalarIndexDefinition;

        let directory = TempDir::new().unwrap();
        let store = RustStore::new(
            directory.path().join("group-conflict.owl"),
            10 * 1024 * 1024,
        )
        .unwrap();
        store
            .upsert_scalar_index(ScalarIndexDefinition {
                name: "events_by_fingerprint".to_string(),
                schema: "events".to_string(),
                fields: vec!["fingerprint".to_string()],
                unique: true,
            })
            .unwrap();
        let request = |id: &str, fingerprint: &str| {
            let (reply, response) = mpsc::channel();
            (
                Request {
                    documents: vec![Document::with_id(
                        id.to_string(),
                        "events".to_string(),
                        HashMap::from([(
                            "fingerprint".to_string(),
                            Value::String(fingerprint.to_string()),
                        )]),
                    )],
                    reply,
                },
                response,
            )
        };
        let (first, first_response) = request("first", "same");
        let (conflict, conflict_response) = request("conflict", "same");
        let (independent, independent_response) = request("independent", "different");

        commit_cohort(&store, vec![first, conflict, independent]);

        assert!(first_response.recv().unwrap().is_ok());
        assert!(conflict_response.recv().unwrap().is_err());
        assert!(independent_response.recv().unwrap().is_ok());
        let ids = store
            .scan_documents("events")
            .unwrap()
            .into_iter()
            .map(|document| document.id)
            .collect::<HashSet<_>>();
        assert_eq!(
            ids,
            HashSet::from(["first".to_string(), "independent".to_string()])
        );
    }
}
