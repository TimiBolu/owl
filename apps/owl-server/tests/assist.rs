//! What the server gives a program that writes queries: the catalogue, a
//! check, reads that cannot write, a budget, and all of them as tools
//! over the Model Context Protocol.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use owl_db::storage::RustStore;
use owl_server::{router, Options};
use serde_json::{json, Value};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

struct Service {
    _directory: TempDir,
    app: Router,
}

impl Service {
    async fn start(options: Options) -> Self {
        let directory = TempDir::new().unwrap();
        let store = Arc::new(RustStore::new(directory.path().join("db.owl"), 16 << 20).unwrap());
        let (app, _janitor) = router(store, options);
        let service = Self { _directory: directory, app };
        service
            .ok(
                "/v1/script",
                json!({ "source": r#"
                    schema people { id: String, name: String, level: Integer, manager: Optional(Ref(people)) as reports }
                    schema projects { id: String, name: String, owner: Ref(people) as owned }
                    people.insert [
                      { id: "kim", name: "Kim", level: 3 }, { id: "ada", name: "Ada", level: 1, manager: "kim" },
                      { id: "ben", name: "Ben", level: 2, manager: "kim" }
                    ]
                    projects.insert [ { id: "apollo", name: "Apollo", owner: "kim" } ]
                "# }),
            )
            .await;
        service
    }

    async fn send(&self, method: &str, path: &str, body: Option<Value>, headers: &[(&str, &str)]) -> (StatusCode, Value) {
        let mut request = Request::builder().method(method).uri(path);
        for (name, value) in headers {
            request = request.header(*name, *value);
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
        (status, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send("POST", path, Some(body), &[]).await
    }

    async fn ok(&self, path: &str, body: Value) -> Value {
        let (status, answer) = self.post(path, body.clone()).await;
        assert_eq!(status, StatusCode::OK, "{path} {body} => {answer}");
        answer
    }

    async fn count(&self, query: &str) -> usize {
        self.ok("/v1/query", json!({ "query": query })).await["rows"].as_array().unwrap().len()
    }

    /// Call a tool, and give its text and whether it reported a failure.
    async fn tool(&self, name: &str, arguments: Value) -> (String, bool) {
        let answer = self
            .ok(
                "/mcp",
                json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": arguments } }),
            )
            .await;
        assert_eq!(answer["id"], 7, "{answer}");
        let result = &answer["result"];
        assert_eq!(result["content"][0]["type"], "text", "{answer}");
        (
            result["content"][0]["text"].as_str().unwrap().to_string(),
            result["isError"].as_bool().unwrap(),
        )
    }
}

#[tokio::test]
async fn the_catalogue_says_what_is_there_as_data_and_as_text() {
    let service = Service::start(Options::default()).await;
    let (status, catalogue) = service.send("GET", "/v1/catalogue", None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    let people = &catalogue["collections"][0];
    assert_eq!(people["name"], "people");
    assert_eq!(people["rows"], 3);
    assert_eq!(people["fields"][2], json!({ "name": "manager", "type": "Optional(Ref(people))", "refers_to": "people" }));
    assert_eq!(
        people["relationships"],
        json!([
            { "name": "owned", "from": "projects", "by": "owner" },
            { "name": "reports", "from": "people", "by": "manager" }
        ])
    );
    let text = catalogue["text"].as_str().unwrap();
    assert!(text.contains("schema people {  // 3 rows"), "{text}");
    assert!(text.contains("// .reports: the people whose .manager is this row"), "{text}");
}

#[tokio::test]
async fn a_check_answers_without_running_and_a_failed_query_carries_the_same_problem() {
    let service = Service::start(Options::default()).await;
    let passed = service.ok("/v1/check", json!({ "query": "people.where { .level < $most }.delete()" })).await;
    assert_eq!(passed, json!({ "ok": true, "collection": "people", "writes": true, "parameters": ["most"] }));
    assert_eq!(service.count("people").await, 3);

    let refused = service.ok("/v1/check", json!({ "query": "people.where { .levle > 1 }" })).await;
    assert_eq!(refused["ok"], false);
    assert_eq!(refused["problem"]["code"], "unknown_name");
    assert_eq!(refused["problem"]["suggestion"], "level");
    assert_eq!(refused["problem"]["names"]["people"], json!(["id", "level", "manager", "name", "owned", "reports"]));

    // Run, the same statement fails with the code it always had, and now
    // with the problem beside it.
    let (status, failed) = service.post("/v1/query", json!({ "query": "people.where { .levle > 1 }" })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(failed["error"]["code"], "rejected");
    assert_eq!(failed["error"]["problem"], refused["problem"]);

    let (status, failed) = service.post("/v1/query", json!({ "query": "people.where { .level > }" })).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(failed["error"]["code"], "parse_error");
    assert_eq!(failed["error"]["problem"]["column"], 25);
}

#[tokio::test]
async fn a_read_only_query_cannot_write_and_a_budget_stops_one() {
    let service = Service::start(Options::default()).await;
    let read = service
        .ok("/v1/query", json!({ "query": "people.where { .level > 1 }.select { name }", "read_only": true }))
        .await;
    assert_eq!(read["rows"], json!([{ "name": "Ben" }, { "name": "Kim" }]));

    for write in [
        "people.insert { id: \"zed\", name: \"Zed\", level: 2 }",
        "people.where { .level < 3 }.update { level += 1 }",
        "people.delete()",
    ] {
        let (status, refused) = service.post("/v1/query", json!({ "query": write, "read_only": true })).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{write}");
        assert_eq!(refused["error"]["code"], "read_only", "{write}");
    }
    assert_eq!(service.count("people").await, 3);
    assert_eq!(service.count("people.where { .level == 1 }").await, 1);

    // Three people paired with one another twice over is 27 rows.
    let pairs = "people { a in people  b in people  select { id, a: a.id, b: b.id } }";
    assert_eq!(service.count(pairs).await, 27);
    let (status, stopped) = service.post("/v1/query", json!({ "query": pairs, "max_rows": 5 })).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(stopped["error"]["code"], "over_budget");
    assert_eq!(stopped["error"]["problem"]["code"], "over_budget");
    let (_, stopped) = service.post("/v1/query", json!({ "query": pairs, "max_ms": 0, "read_only": true })).await;
    assert_eq!(stopped["error"]["code"], "over_budget");

    // The server's own budget holds whatever a request asks for.
    let strict = Service::start(Options {
        budget: owl_db::Budget { rows: Some(5), time: None },
        ..Options::default()
    })
    .await;
    let (_, stopped) = strict.post("/v1/query", json!({ "query": pairs, "max_rows": 1000 })).await;
    assert_eq!(stopped["error"]["code"], "over_budget");
}

#[tokio::test]
async fn a_model_is_introduced_to_the_tools_and_uses_them() {
    let service = Service::start(Options::default()).await;
    let opened = service
        .ok(
            "/mcp",
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {
                "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "test", "version": "0" }
            } }),
        )
        .await;
    assert_eq!(opened["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(opened["result"]["capabilities"], json!({ "tools": {} }));
    assert_eq!(opened["result"]["serverInfo"]["name"], "owl");
    assert!(opened["result"]["instructions"].as_str().unwrap().contains("not SQL"));

    // A version this does not know is answered with the newest it does.
    let other = service
        .ok("/mcp", json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "1999-01-01" } }))
        .await;
    assert_eq!(other["result"]["protocolVersion"], "2025-11-25");

    // A notification is taken and not answered.
    let (status, body) = service.post("/mcp", json!({ "jsonrpc": "2.0", "method": "notifications/initialized" })).await;
    assert_eq!((status, body), (StatusCode::ACCEPTED, Value::Null));
    assert_eq!(service.ok("/mcp", json!({ "jsonrpc": "2.0", "id": 2, "method": "ping" })).await["result"], json!({}));
    assert_eq!(service.send("GET", "/mcp", None, &[]).await.0, StatusCode::METHOD_NOT_ALLOWED);

    let listed = service.ok("/mcp", json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/list" })).await;
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| {
            assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
            assert!(tool["description"].as_str().unwrap().len() > 40, "{tool}");
            tool["name"].as_str().unwrap()
        })
        .collect();
    assert_eq!(names, ["language", "describe", "check", "explain", "query"]);

    let (language, failed) = service.tool("language", json!({})).await;
    assert!(!failed && language.starts_with("# OQL on one page"));
    let (described, failed) = service.tool("describe", json!({})).await;
    assert!(!failed && described.contains("// .owned: the projects whose .owner is this row"), "{described}");

    // A slip, the correction the check offers, and the answer.
    let (refused, failed) = service.tool("check", json!({ "query": "people.where { .levle > 1 }" })).await;
    assert!(failed);
    let problem: Value = serde_json::from_str(&refused).unwrap();
    assert_eq!(problem["suggestion"], "level");
    let (passed, failed) = service.tool("check", json!({ "query": "people.where { .level > 1 }" })).await;
    assert!(!failed, "{passed}");
    let (answer, failed) = service
        .tool("query", json!({ "query": "people.where { .level > $over }.select { name }", "params": { "over": 1 } }))
        .await;
    assert!(!failed, "{answer}");
    assert_eq!(
        serde_json::from_str::<Value>(&answer).unwrap(),
        json!({ "rows": [{ "name": "Ben" }, { "name": "Kim" }], "row_count": 2 })
    );
    let (answer, _) = service.tool("query", json!({ "query": "people", "limit": 2 })).await;
    let answer: Value = serde_json::from_str(&answer).unwrap();
    assert_eq!((answer["rows"].as_array().unwrap().len(), &answer["row_count"]), (2, &json!(3)));
    assert!(answer["note"].as_str().unwrap().contains("the first 2 of 3 rows"));

    let (plan, failed) = service
        .tool("explain", json!({ "query": "people { r in .reports  select { name, r: r.name } }" }))
        .await;
    assert!(!failed && plan.contains("r in .reports: 3 rows in, 2 out"), "{plan}");

    // Nothing sent here writes.
    let (refused, failed) = service.tool("query", json!({ "query": "people.delete()" })).await;
    assert!(failed && refused.contains("read_only"), "{refused}");
    assert_eq!(service.count("people").await, 3);

    let unknown = service
        .ok("/mcp", json!({ "jsonrpc": "2.0", "id": 9, "method": "tools/call", "params": { "name": "drop_everything" } }))
        .await;
    assert_eq!(unknown["error"]["code"], -32602);
    let unknown = service.ok("/mcp", json!({ "jsonrpc": "2.0", "id": 9, "method": "resources/list" })).await;
    assert_eq!(unknown["error"]["code"], -32601);
}

#[tokio::test]
async fn a_model_s_query_is_held_to_its_budget_and_a_page_elsewhere_is_refused() {
    let service = Service::start(Options {
        assistant_budget: owl_db::Budget { rows: Some(5), time: None },
        ..Options::default()
    })
    .await;
    let (stopped, failed) = service
        .tool("query", json!({ "query": "people { a in people  b in people  select { id, a: a.id, b: b.id } }" }))
        .await;
    assert!(failed);
    let problem: Value = serde_json::from_str(&stopped).unwrap();
    assert_eq!(problem["code"], "over_budget");
    assert!(problem["message"].as_str().unwrap().contains("Filter earlier"), "{stopped}");

    let ping = json!({ "jsonrpc": "2.0", "id": 1, "method": "ping" });
    for (origin, expected) in [
        ("https://elsewhere.example", StatusCode::FORBIDDEN),
        ("http://localhost:3000", StatusCode::OK),
        ("http://127.0.0.1:7878", StatusCode::OK),
    ] {
        let (status, _) = service.send("POST", "/mcp", Some(ping.clone()), &[("origin", origin)]).await;
        assert_eq!(status, expected, "{origin}");
    }
    let (status, _) = service
        .send("POST", "/mcp", Some(ping.clone()), &[("mcp-protocol-version", "1999-01-01")])
        .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
