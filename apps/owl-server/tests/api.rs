//! The server's contract, exercised through its router.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use owl_db::storage::RustStore;
use owl_server::{router, Janitor, Options};
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tower::ServiceExt;

struct Service {
    _directory: TempDir,
    app: Router,
    janitor: Janitor,
    token: Option<String>,
}

impl Service {
    fn start(options: Options) -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
        let token = options.token.clone();
        let (app, janitor) = router(store, options);
        Self {
            _directory: directory,
            app,
            janitor,
            token,
        }
    }

    async fn with_shop() -> Self {
        let service = Self::start(Options::default());
        let (status, body) = service
            .post(
                "/v1/script",
                json!({ "source": r#"
                    schema customers { id: String, name: String, email: String unique, joined: DateTime }
                    schema orders {
                      id: String,
                      customer: Ref(customers),
                      total: Float,
                      stock: Integer = 0,
                      items: List({ sku: String, qty: Integer }),
                      note: Optional(String),
                      index(.total)
                    }
                "# }),
            )
            .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["schemas"], 2);
        service
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>, token: Option<&str>) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        let request = match body {
            Some(body) => request
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
            None => request.body(Body::empty()).unwrap(),
        };
        let response = self.app.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body = serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| json!({ "text": String::from_utf8_lossy(&bytes).to_string() }));
        (status, body)
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send("POST", path, Some(body), self.token.as_deref()).await
    }

    async fn ok(&self, path: &str, body: Value) -> Value {
        let (status, answer) = self.post(path, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{path} {body} => {answer}");
        answer
    }

    async fn rows(&self, query: &str) -> Vec<Value> {
        self.ok("/v1/query", json!({ "query": query })).await["rows"]
            .as_array()
            .unwrap()
            .clone()
    }

    async fn begin(&self) -> u64 {
        self.ok("/v1/transactions", json!({})).await["transaction"]
            .as_u64()
            .unwrap()
    }

    async fn seed(&self) {
        let inserted = self
            .ok(
                "/v1/insert",
                json!({ "collection": "customers", "documents": [
                    { "id": "c1", "name": "Ada", "email": "ada@x.org", "joined": "2026-01-05T10:00:00Z" },
                    { "id": "c2", "name": "Bo", "email": "bo@x.org", "joined": "2026-02-01" }
                ] }),
            )
            .await;
        assert_eq!(inserted, json!({ "inserted": 2, "ids": ["c1", "c2"] }));
        self.ok(
            "/v1/insert",
            json!({ "collection": "orders", "documents": [
                { "id": "o1", "customer": "c1", "total": 12.5, "stock": 10, "items": [{ "sku": "a", "qty": 2 }] },
                { "id": "o2", "customer": "c2", "total": 40.0, "stock": 10, "items": [], "note": "gift" }
            ] }),
        )
        .await;
    }
}

fn code(body: &Value) -> &str {
    body["error"]["code"].as_str().unwrap_or("")
}

#[tokio::test]
async fn health_is_open_and_everything_else_needs_the_token() {
    let service = Service::start(Options {
        token: Some("s3cret".to_string()),
        ..Options::default()
    });
    let (status, body) = service.send("GET", "/v1/health", None, None).await;
    assert_eq!((status, body["status"].as_str()), (StatusCode::OK, Some("ok")));

    let query = json!({ "query": "things.count()" });
    for token in [None, Some("wrong"), Some("s3cre"), Some("s3cret2")] {
        let (status, body) = service.send("POST", "/v1/query", Some(query.clone()), token).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{token:?}");
        assert_eq!(code(&body), "unauthorized");
    }
    let (status, _) = service.send("POST", "/v1/transactions", Some(json!({})), None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // With the token the request reaches the database.
    let (status, body) = service.send("POST", "/v1/transactions", Some(json!({})), Some("s3cret")).await;
    assert_eq!(status, StatusCode::OK, "{body}");
}

#[tokio::test]
async fn documents_go_in_as_json_and_come_back_as_json() {
    let service = Service::with_shop().await;
    service.seed().await;

    // A reference comes back as the id it was written as, a timestamp as
    // RFC 3339 text, and nested values as they were.
    let rows = service.rows(r#"orders.where { .id == "o1" }"#).await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["customer"], "c1");
    assert_eq!(rows[0]["total"], 12.5);
    assert_eq!(rows[0]["items"], json!([{ "sku": "a", "qty": 2 }]));
    let rows = service.rows(r#"customers.sort { .id.asc }.select { id, joined }"#).await;
    assert_eq!(rows[0]["joined"], "2026-01-05T10:00:00+00:00");
    assert_eq!(rows[1]["joined"], "2026-02-01T00:00:00+00:00");

    // Navigation, aggregation and absent values.
    let rows = service
        .rows(r#"orders.sort { .id.asc }.select { id, who: .customer.name, note, lines: .items.count() }"#)
        .await;
    assert_eq!(
        rows,
        vec![
            json!({ "id": "o1", "who": "Ada", "note": null, "lines": 1 }),
            json!({ "id": "o2", "who": "Bo", "note": "gift", "lines": 0 })
        ]
    );
    assert_eq!(service.rows("orders.sum(.total)").await[0]["sum"], 52.5);
    assert_eq!(
        service.rows(r#"customers.where { matches(.email, "^ada@") }.select { id }"#).await,
        vec![json!({ "id": "c1" })]
    );
}

#[tokio::test]
async fn values_travel_as_parameters_not_as_query_text() {
    let service = Service::with_shop().await;
    service.seed().await;
    let answer = service
        .ok(
            "/v1/query",
            json!({
                "query": "orders.where { .total >= $least && .customer == $who }.select { id }",
                "params": { "least": 10, "who": "c2" }
            }),
        )
        .await;
    assert_eq!(answer["rows"], json!([{ "id": "o2" }]));

    // Text that would be OQL if pasted into the query is only text here.
    let hostile = r#"x" } .delete //"#;
    let answer = service
        .ok(
            "/v1/query",
            json!({ "query": "customers.where { .name == $name }.count()", "params": { "name": hostile } }),
        )
        .await;
    assert_eq!(answer["rows"][0]["count"], 0);
    assert_eq!(service.rows("customers.count()").await[0]["count"], 2);

    // Updating with parameters, including a list and an object.
    service
        .ok(
            "/v1/query",
            json!({
                "query": r#"orders.where { .id == $id }.update { note: $note, items: $items }"#,
                "params": { "id": "o1", "note": "it's \"quoted\"", "items": [{ "sku": "z", "qty": 9 }] }
            }),
        )
        .await;
    let rows = service.rows(r#"orders.where { .id == "o1" }.select { note, items }"#).await;
    assert_eq!(rows[0], json!({ "note": "it's \"quoted\"", "items": [{ "sku": "z", "qty": 9 }] }));

    let (status, body) = service
        .post("/v1/query", json!({ "query": "orders.where { .id == $id }" }))
        .await;
    assert_eq!((status, code(&body)), (StatusCode::BAD_REQUEST, "invalid_request"), "{body}");
}

#[tokio::test]
async fn what_owl_refuses_is_reported_with_a_code() {
    let service = Service::with_shop().await;
    service.seed().await;
    let cases = [
        ("/v1/query", json!({ "query": "orders.where {" }), StatusCode::BAD_REQUEST, "parse_error"),
        ("/v1/query", json!({ "query": "orders.select { nope }" }), StatusCode::UNPROCESSABLE_ENTITY, "rejected"),
        ("/v1/query", json!({ "query": r#"customers.where { matches(.name, "(") }"# }), StatusCode::UNPROCESSABLE_ENTITY, "rejected"),
        (
            "/v1/insert",
            json!({ "collection": "orders", "documents": [{ "id": "o9", "customer": "nobody", "total": 1.0, "items": [] }] }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "rejected",
        ),
        (
            "/v1/insert",
            json!({ "collection": "customers", "documents": [{ "id": "c1", "name": "Dup", "email": "d@x.org", "joined": "2026-01-01" }] }),
            StatusCode::UNPROCESSABLE_ENTITY,
            "rejected",
        ),
        ("/v1/script", json!({ "source": "schema broken {" }), StatusCode::BAD_REQUEST, "parse_error"),
        ("/v1/transactions/999/commit", json!({}), StatusCode::NOT_FOUND, "no_such_transaction"),
        ("/v1/transactions/999/rollback", json!({}), StatusCode::NOT_FOUND, "no_such_transaction"),
        ("/v1/query", json!({ "query": "orders.count()", "transaction": 999 }), StatusCode::NOT_FOUND, "no_such_transaction"),
    ];
    for (path, body, status, expected) in cases {
        let (got, answer) = service.post(path, body.clone()).await;
        assert_eq!((got, code(&answer)), (status, expected), "{path} {body} => {answer}");
        assert!(!answer["error"]["message"].as_str().unwrap().is_empty());
    }
    // A request that is not the shape the endpoint takes never reaches Owl.
    let (status, _) = service.post("/v1/query", json!({ "querry": "orders" })).await;
    assert!(status.is_client_error());
    // Nothing above changed anything.
    assert_eq!(service.rows("orders.count()").await[0]["count"], 2);
    assert_eq!(service.rows("customers.count()").await[0]["count"], 2);
}

#[tokio::test]
async fn a_transaction_is_private_until_it_commits() {
    let service = Service::with_shop().await;
    service.seed().await;
    let transaction = service.begin().await;

    service
        .ok(
            "/v1/insert",
            json!({ "collection": "customers", "transaction": transaction, "documents": [
                { "id": "c3", "name": "Cy", "email": "cy@x.org", "joined": "2026-03-01" }
            ] }),
        )
        .await;
    service
        .ok(
            "/v1/query",
            json!({ "transaction": transaction, "query": r#"orders.where { .id == "o1" }.update { total: 99.0 }"# }),
        )
        .await;

    let inside = service
        .ok("/v1/query", json!({ "transaction": transaction, "query": "customers.count()" }))
        .await;
    assert_eq!(inside["rows"][0]["count"], 3);
    assert_eq!(service.rows("customers.count()").await[0]["count"], 2);
    assert_eq!(service.rows(r#"orders.where { .id == "o1" }.select { total }"#).await[0]["total"], 12.5);

    let committed = service.ok(&format!("/v1/transactions/{transaction}/commit"), json!({})).await;
    assert_eq!(committed["committed"], true);
    assert_eq!(service.rows("customers.count()").await[0]["count"], 3);
    assert_eq!(service.rows(r#"orders.where { .id == "o1" }.select { total }"#).await[0]["total"], 99.0);

    // It is over: it cannot be used, committed or rolled back again.
    for path in [
        format!("/v1/transactions/{transaction}/commit"),
        format!("/v1/transactions/{transaction}/rollback"),
    ] {
        let (status, body) = service.post(&path, json!({})).await;
        assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "no_such_transaction"));
    }

    // Rolling back leaves nothing.
    let abandoned = service.begin().await;
    service
        .ok(
            "/v1/query",
            json!({ "transaction": abandoned, "query": r#"orders.where { .id == "o2" }.delete"# }),
        )
        .await;
    service.ok(&format!("/v1/transactions/{abandoned}/rollback"), json!({})).await;
    assert_eq!(service.rows("orders.count()").await[0]["count"], 2);
}

#[tokio::test]
async fn the_second_commit_to_one_document_is_a_conflict() {
    let service = Service::with_shop().await;
    service.seed().await;
    let first = service.begin().await;
    let second = service.begin().await;
    for (transaction, total) in [(first, 1.0), (second, 2.0)] {
        service
            .ok(
                "/v1/query",
                json!({
                    "transaction": transaction,
                    "query": r#"orders.where { .id == "o1" }.update { total: $total }"#,
                    "params": { "total": total }
                }),
            )
            .await;
    }
    service.ok(&format!("/v1/transactions/{first}/commit"), json!({})).await;
    let (status, body) = service.post(&format!("/v1/transactions/{second}/commit"), json!({})).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "conflict"), "{body}");
    assert_eq!(service.rows(r#"orders.where { .id == "o1" }.select { total }"#).await[0]["total"], 1.0);
}

#[tokio::test]
async fn a_transaction_left_idle_is_rolled_back() {
    let service = Service::start(Options {
        transaction_idle: Duration::from_millis(80),
        ..Options::default()
    });
    service
        .ok("/v1/script", json!({ "source": "schema notes { id: String, text: String }" }))
        .await;
    let idle = service.begin().await;
    service
        .ok(
            "/v1/insert",
            json!({ "collection": "notes", "transaction": idle, "documents": [{ "id": "n1", "text": "lost" }] }),
        )
        .await;
    assert_eq!(service.janitor.expire_idle_transactions(), 0);

    tokio::time::sleep(Duration::from_millis(120)).await;
    let kept = service.begin().await;
    assert_eq!(service.janitor.expire_idle_transactions(), 1);

    let (status, body) = service.post(&format!("/v1/transactions/{idle}/commit"), json!({})).await;
    assert_eq!((status, code(&body)), (StatusCode::NOT_FOUND, "no_such_transaction"));
    assert_eq!(service.rows("notes.count()").await[0]["count"], 0);
    // One that is in use is left alone.
    service.ok(&format!("/v1/transactions/{kept}/commit"), json!({})).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_clients_at_once_lose_no_update() {
    let service = Arc::new(Service::with_shop().await);
    service.seed().await;
    let clients = 12;
    let each = 15;
    let mut tasks = Vec::new();
    for _ in 0..clients {
        let service = service.clone();
        tasks.push(tokio::spawn(async move {
            let mut taken = 0;
            for _ in 0..each {
                // One statement, no transaction: take one from stock if any is left.
                let answer = service
                    .ok(
                        "/v1/query",
                        json!({ "query": r#"orders.where { .id == "o1" && .stock > 0 }.update { stock: .stock - 1 }"# }),
                    )
                    .await;
                taken += answer["rows"].as_array().unwrap().len();
                // And a transaction that reads then writes, retried on conflict.
                loop {
                    let transaction = service.begin().await;
                    let read = service
                        .ok(
                            "/v1/query",
                            json!({ "transaction": transaction, "query": r#"orders.where { .id == "o2" }.select { total }"# }),
                        )
                        .await;
                    let total = read["rows"][0]["total"].as_f64().unwrap();
                    service
                        .ok(
                            "/v1/query",
                            json!({
                                "transaction": transaction,
                                "query": r#"orders.where { .id == "o2" }.update { total: $total }"#,
                                "params": { "total": total + 1.0 }
                            }),
                        )
                        .await;
                    let (status, body) = service
                        .post(&format!("/v1/transactions/{transaction}/commit"), json!({}))
                        .await;
                    match status {
                        StatusCode::OK => break,
                        StatusCode::CONFLICT => continue,
                        other => panic!("{other} {body}"),
                    }
                }
            }
            taken
        }));
    }
    let mut taken = 0;
    for task in tasks {
        taken += task.await.unwrap();
    }
    // Ten in stock: exactly ten were taken, however many asked.
    assert_eq!(taken, 10);
    assert_eq!(service.rows(r#"orders.where { .id == "o1" }.select { stock }"#).await[0]["stock"], 0);
    assert_eq!(
        service.rows(r#"orders.where { .id == "o2" }.select { total }"#).await[0]["total"],
        40.0 + (clients * each) as f64
    );
}

#[tokio::test]
async fn a_backup_is_taken_where_the_server_was_told_and_restores() {
    let backups = TempDir::new().unwrap();
    let service = Service::start(Options {
        backup_directory: Some(backups.path().to_path_buf()),
        ..Options::default()
    });
    service
        .ok("/v1/script", json!({ "source": "schema notes { id: String, text: String }" }))
        .await;
    service
        .ok(
            "/v1/insert",
            json!({ "collection": "notes", "documents": [{ "id": "n1", "text": "kept" }, { "id": "n2", "text": "kept too" }] }),
        )
        .await;

    let first = service.ok("/v1/backups", json!({})).await;
    assert!(first["bytes"].as_u64().unwrap() > 0, "{first}");
    // What happens next is not in it.
    service
        .ok("/v1/query", json!({ "query": r#"notes.where { .id == "n1" }.delete"# }))
        .await;
    let second = service.ok("/v1/backups", json!({})).await;
    assert_ne!(first["name"], second["name"]);
    assert!(second["committed_lsn"].as_u64() > first["committed_lsn"].as_u64());

    // A client cannot say where a backup goes.
    let (status, _) = service.post("/v1/backups", json!({ "directory": "/tmp/elsewhere" })).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(std::fs::read_dir(backups.path()).unwrap().count(), 3);

    // The first backup restores to the database as it was then.
    let home = TempDir::new().unwrap();
    let restored = Arc::new(
        RustStore::restore(
            backups.path().join(first["name"].as_str().unwrap()),
            home.path().join("restored.owl"),
            16 << 20,
        )
        .unwrap(),
    );
    let (app, janitor) = router(restored, Options::default());
    let copy = Service {
        _directory: home,
        app,
        janitor,
        token: None,
    };
    assert_eq!(copy.rows("notes.count()").await[0]["count"], 2);
    assert_eq!(service.rows("notes.count()").await[0]["count"], 1);

    // A server started without a backup directory takes none.
    let plain = Service::start(Options::default());
    let (status, body) = plain.post("/v1/backups", json!({})).await;
    assert_eq!((status, code(&body)), (StatusCode::CONFLICT, "backups_disabled"));
}
