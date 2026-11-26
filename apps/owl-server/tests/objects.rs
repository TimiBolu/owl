//! Servers that keep a database in an object store: one that writes it,
//! one that reads it, and one that takes it over.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use owl_db::storage::RustStore;
use owl_server::{open_in_objects, router, ObjectOptions, Objects, Options, Role};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

/// A server over the database at `directory`, with its parts in `bucket`.
struct Service {
    store: Arc<RustStore>,
    app: Router,
}

impl Service {
    fn open(directory: &Path, bucket: &Path, role: Role) -> Result<Self, String> {
        let store = open_in_objects(
            &directory.join("shop.owl"),
            64 << 20,
            &ObjectOptions {
                objects: Objects::Directory(bucket.to_path_buf()),
                role,
                cache_bytes: 8 << 20,
            },
        )?;
        let (app, _) = router(store.clone(), Options::default());
        Ok(Self { store, app })
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let request = Request::builder()
            .method("POST")
            .uri(path)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "text": String::from_utf8_lossy(&bytes).to_string() }));
        (status, body)
    }

    async fn insert(&self, ids: std::ops::Range<u32>) {
        let documents: Vec<Value> = ids.map(|n| json!({ "id": format!("n{n:04}"), "n": n })).collect();
        let (status, body) = self.post("/v1/insert", json!({ "collection": "notes", "documents": documents })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
    }

    async fn count(&self) -> u64 {
        let (status, body) = self.post("/v1/query", json!({ "query": "notes.count()" })).await;
        assert_eq!(status, StatusCode::OK, "{body}");
        body["rows"][0]["count"].as_u64().unwrap_or_else(|| panic!("no count in {body}"))
    }
}

#[tokio::test]
async fn a_reader_serves_what_the_writer_checkpointed_and_takes_no_writes() {
    let (bucket, here, there) = (TempDir::new().unwrap(), TempDir::new().unwrap(), TempDir::new().unwrap());
    let writer = Service::open(here.path(), bucket.path(), Role::Writer).unwrap();
    writer.insert(0..300).await;

    // Nothing is published until a checkpoint, so there is nothing to read.
    let early = Service::open(there.path(), bucket.path(), Role::Reader).err().unwrap();
    assert!(early.contains("has not been published"), "{early}");

    // A checkpoint publishes, without being asked to.
    writer.store.checkpoint().unwrap();
    writer.insert(300..320).await;

    let reader = Service::open(there.path(), bucket.path(), Role::Reader).unwrap();
    assert_eq!(reader.count().await, 300, "a reader sees the last checkpoint");
    assert_eq!(writer.count().await, 320);

    // A reader's copy is to be read.
    let (status, body) = reader
        .post("/v1/insert", json!({ "collection": "notes", "documents": [{ "id": "mine", "n": 1 }] }))
        .await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert!(body.to_string().contains("read-only"), "{body}");
    let (status, body) = reader.post("/v1/query", json!({ "query": "notes.delete()" })).await;
    assert_ne!(status, StatusCode::OK, "{body}");
    assert_eq!(reader.count().await, 300);

    // Opened again after the next checkpoint, it sees more.
    writer.store.checkpoint().unwrap();
    drop(reader);
    let reader = Service::open(there.path(), bucket.path(), Role::Reader).unwrap();
    assert_eq!(reader.count().await, 320);
}

#[tokio::test]
async fn a_second_server_cannot_write_the_database_unless_it_takes_it_over() {
    let (bucket, here, there) = (TempDir::new().unwrap(), TempDir::new().unwrap(), TempDir::new().unwrap());
    let first = Service::open(here.path(), bucket.path(), Role::Writer).unwrap();
    first.insert(0..100).await;
    first.store.checkpoint().unwrap();
    first.insert(100..110).await;

    let refused = Service::open(there.path(), bucket.path(), Role::Writer).err().unwrap();
    assert!(refused.contains("claimed by another writer"), "{refused}");

    // Taking over starts from the publication: the ten rows the first
    // server had not checkpointed do not cross.
    let second = Service::open(there.path(), bucket.path(), Role::TakeOver).unwrap();
    assert_eq!(second.count().await, 100);
    second.insert(200..250).await;
    second.store.checkpoint().unwrap();

    // The first server still answers from its own disk, and its next
    // publication is refused. The second's database is what readers get.
    first.insert(110..120).await;
    first.store.checkpoint().unwrap();
    let status = first.store.maintenance_status().unwrap();
    assert!(status.last_error.is_some_and(|error| error.contains("taken over")));

    let reading = TempDir::new().unwrap();
    let reader = Service::open(reading.path(), bucket.path(), Role::Reader).unwrap();
    assert_eq!(reader.count().await, 150);

    // And the first server cannot come back as the writer.
    drop(first);
    let refused = Service::open(here.path(), bucket.path(), Role::Writer).err().unwrap();
    assert!(refused.contains("claimed by another writer"), "{refused}");
}
