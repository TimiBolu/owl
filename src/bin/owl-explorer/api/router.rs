use owl_db::parser::parse_oql;
use serde_json::json;
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::net::TcpStream;
use std::path::Path;

use crate::api::handlers::*;
use crate::api::models::*;
use crate::api::utils::{content_type_for_path, project_root, send_bytes, send_html, send_json};

fn ui_dist_dir() -> std::path::PathBuf {
    project_root().join("owl_explorer").join("dist")
}

fn ui_dev_index() -> std::path::PathBuf {
    project_root().join("owl_explorer").join("index.html")
}

fn read_ui_index() -> String {
    let dist_index = ui_dist_dir().join("index.html");
    if let Ok(raw) = fs::read_to_string(&dist_index) {
        return raw;
    }
    let dev_index = ui_dev_index();
    fs::read_to_string(&dev_index).unwrap_or_else(|_| {
        "<h1>Owl Explorer</h1><p>UI file missing. Build UI via: (cd owl_explorer && npm install && npm run build)</p>".to_string()
    })
}

fn serve_static_asset(stream: &mut TcpStream, request_path: &str) -> bool {
    let path_without_query = request_path.split('?').next().unwrap_or(request_path);
    let trimmed = path_without_query.trim_start_matches('/');
    if trimmed.is_empty() {
        return false;
    }
    if trimmed.contains("..") || trimmed.starts_with("api/") {
        return false;
    }

    let file_path = ui_dist_dir().join(trimmed);
    if !file_path.exists() || !file_path.is_file() {
        return false;
    }
    match fs::read(&file_path) {
        Ok(bytes) => {
            send_bytes(stream, "200 OK", content_type_for_path(&file_path), &bytes);
            true
        }
        Err(_) => false,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ApiRole {
    Readonly,
    Readwrite,
    Admin,
}

fn role_allows(current: ApiRole, required: ApiRole) -> bool {
    current >= required
}

fn auth_is_enabled() -> bool {
    std::env::var("OWL_EXPLORER_READONLY_TOKEN").is_ok()
        || std::env::var("OWL_EXPLORER_READWRITE_TOKEN").is_ok()
        || std::env::var("OWL_EXPLORER_ADMIN_TOKEN").is_ok()
}

fn parse_bearer_token(headers: &HashMap<String, String>) -> Option<String> {
    if let Some(token) = headers.get("x-owl-token") {
        return Some(token.trim().to_string());
    }
    let auth = headers.get("authorization")?;
    let lower = auth.to_ascii_lowercase();
    if lower.starts_with("bearer ") && auth.len() >= 7 {
        return Some(auth[7..].trim().to_string());
    }
    None
}

fn authenticated_role(headers: &HashMap<String, String>) -> Option<ApiRole> {
    let token = parse_bearer_token(headers)?;
    if std::env::var("OWL_EXPLORER_ADMIN_TOKEN")
        .ok()
        .as_deref()
        .map(|v| v == token)
        .unwrap_or(false)
    {
        return Some(ApiRole::Admin);
    }
    if std::env::var("OWL_EXPLORER_READWRITE_TOKEN")
        .ok()
        .as_deref()
        .map(|v| v == token)
        .unwrap_or(false)
    {
        return Some(ApiRole::Readwrite);
    }
    if std::env::var("OWL_EXPLORER_READONLY_TOKEN")
        .ok()
        .as_deref()
        .map(|v| v == token)
        .unwrap_or(false)
    {
        return Some(ApiRole::Readonly);
    }
    None
}

fn required_role_for_route(method: &str, path: &str) -> Option<ApiRole> {
    if !path.starts_with("/api/") {
        return None;
    }
    if method == "GET" && path == "/api/databases" {
        return Some(ApiRole::Readonly);
    }

    match path {
        "/api/databases" | "/api/databases/delete" => Some(ApiRole::Admin),
        "/api/collections/upsert"
        | "/api/collections/delete"
        | "/api/indexes/rebuild"
        | "/api/vector/indexes/upsert"
        | "/api/vector/indexes/delete"
        | "/api/vector/indexes/build"
        | "/api/vector/indexes/rebuild" => Some(ApiRole::Admin),
        "/api/query" | "/api/query/explain" => Some(ApiRole::Readwrite),
        "/api/documents/update"
        | "/api/documents/delete"
        | "/api/documents/delete_all"
        | "/api/documents/duplicate"
        | "/api/import"
        | "/api/seed_demo" => Some(ApiRole::Readwrite),
        _ => Some(ApiRole::Readonly),
    }
}

fn parse_request(
    mut stream: &TcpStream,
) -> Result<(String, String, String, HashMap<String, String>), String> {
    let mut buf = [0u8; 1024 * 1024];
    let n = stream.read(&mut buf).map_err(|e| e.to_string())?;
    if n == 0 {
        return Err("empty request".to_string());
    }
    let raw = String::from_utf8_lossy(&buf[..n]).to_string();

    let mut lines = raw.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| "missing request line".to_string())?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();

    let mut content_length = 0usize;
    let mut headers = HashMap::new();
    for line in raw.split("\r\n").skip(1) {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key_lower = key.trim().to_ascii_lowercase();
            let value_trimmed = value.trim().to_string();
            if key_lower == "content-length" {
                content_length = value_trimmed.parse::<usize>().unwrap_or(0);
            }
            headers.insert(key_lower, value_trimmed);
        }
    }

    let body = if let Some(idx) = raw.find("\r\n\r\n") {
        let body_start = idx + 4;
        let existing = &raw[body_start..];
        if existing.len() >= content_length {
            existing[..content_length].to_string()
        } else {
            existing.to_string()
        }
    } else {
        String::new()
    };

    Ok((method, path, body, headers))
}

pub fn handle_connection(mut stream: TcpStream, registry: &mut Registry, registry_file: &Path) {
    let (method, path, body, headers) = match parse_request(&stream) {
        Ok(req) => req,
        Err(e) => {
            send_json(
                &mut stream,
                "400 Bad Request",
                json!({"ok": false, "error": e}),
            );
            return;
        }
    };

    if let Some(required_role) = required_role_for_route(&method, &path) {
        if auth_is_enabled() {
            match authenticated_role(&headers) {
                Some(role) if role_allows(role, required_role) => {}
                Some(_) => {
                    send_json(
                        &mut stream,
                        "403 Forbidden",
                        json!({
                            "ok": false,
                            "error": "forbidden: insufficient role for route",
                            "error_code": "FORBIDDEN",
                        }),
                    );
                    return;
                }
                None => {
                    send_json(
                        &mut stream,
                        "401 Unauthorized",
                        json!({
                            "ok": false,
                            "error": "unauthorized: missing or invalid token",
                            "error_code": "AUTH_REQUIRED",
                        }),
                    );
                    return;
                }
            }
        }
    }

    match (method.as_str(), path.as_str()) {
        ("GET", "/") => {
            let ui_html = read_ui_index();
            send_html(&mut stream, "200 OK", &ui_html);
        }
        ("GET", p) if !p.starts_with("/api/") => {
            if !serve_static_asset(&mut stream, p) {
                let ui_html = read_ui_index();
                send_html(&mut stream, "200 OK", &ui_html);
            }
        }
        ("GET", "/api/databases") => {
            send_json(
                &mut stream,
                "200 OK",
                json!({"ok": true, "databases": registry.databases}),
            );
        }
        ("POST", "/api/databases") => {
            let req: Result<CreateDbRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match create_database(req, registry, registry_file) {
                    Ok(_) => send_json(&mut stream, "200 OK", json!({"ok": true})),
                    Err(e) => send_json(
                        &mut stream,
                        "400 Bad Request",
                        json!({"ok": false, "error": e}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/databases/delete") => {
            let req: Result<DeleteDatabaseRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match delete_database(req, registry, registry_file) {
                    Ok(payload) => send_json(
                        &mut stream,
                        "200 OK",
                        json!({"ok": true, "result": payload}),
                    ),
                    Err(e) => send_json(
                        &mut stream,
                        "400 Bad Request",
                        json!({"ok": false, "error": e}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/query") => {
            let req: Result<QueryRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let db = registry.get(&req.database);
                    match db {
                        Some(db) => match execute_query(db, &req.query) {
                            Ok(payload) => {
                                let ok =
                                    payload.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
                                if ok {
                                    send_json(&mut stream, "200 OK", payload)
                                } else {
                                    send_json(&mut stream, "400 Bad Request", payload)
                                }
                            }
                            Err(e) => send_json(
                                &mut stream,
                                "400 Bad Request",
                                json!({"ok": false, "error": e}),
                            ),
                        },
                        None => send_json(
                            &mut stream,
                            "404 Not Found",
                            json!({"ok": false, "error": "database not found"}),
                        ),
                    }
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/query/explain") => {
            let req: Result<ExplainQueryRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let db = registry.get(&req.database);
                    match db {
                        Some(db) => match explain_query(db, &req.query) {
                            Ok(payload) => send_json(
                                &mut stream,
                                "200 OK",
                                json!({"ok": true, "data": payload}),
                            ),
                            Err(e) => send_json(
                                &mut stream,
                                "400 Bad Request",
                                json!({"ok": false, "error": e}),
                            ),
                        },
                        None => send_json(
                            &mut stream,
                            "404 Not Found",
                            json!({"ok": false, "error": "database not found"}),
                        ),
                    }
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/metrics") => {
            let req: Result<MetricsRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let payload = build_metrics_payload(registry, req.database.as_deref());
                    send_json(
                        &mut stream,
                        "200 OK",
                        json!({ "ok": true, "data": payload }),
                    );
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e), "error_code": "INVALID_PAYLOAD"}),
                ),
            }
        }
        ("POST", "/api/cluster/topology") => {
            let req: Result<MetricsRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let payload = build_cluster_topology_payload(registry, req.database.as_deref());
                    send_json(
                        &mut stream,
                        "200 OK",
                        json!({ "ok": true, "data": payload }),
                    );
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e), "error_code": "INVALID_PAYLOAD"}),
                ),
            }
        }
        ("POST", "/api/ai/chat") => {
            let req: Result<AIChatRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let payload = ai_chat_response(&req.messages);
                    send_json(
                        &mut stream,
                        "200 OK",
                        json!({ "ok": true, "data": payload }),
                    );
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e), "error_code": "INVALID_PAYLOAD"}),
                ),
            }
        }
        ("POST", "/api/ai/generate_query") => {
            let req: Result<AIGenerateQueryRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let prompt = req.prompt.to_lowercase();
                    let query = if prompt.contains("orders") {
                        "orders.select { id, amount, status }"
                    } else if prompt.contains("user") {
                        "users.select { id, name }"
                    } else {
                        "users"
                    };
                    send_json(
                        &mut stream,
                        "200 OK",
                        json!({ "ok": true, "data": { "query": query } }),
                    );
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e), "error_code": "INVALID_PAYLOAD"}),
                ),
            }
        }
        ("POST", "/api/ai/explain_query") => {
            let req: Result<AIExplainQueryRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let explanation = if parse_oql(&req.query).is_ok() {
                        "Query parses successfully and can be executed."
                    } else {
                        "Query does not parse. Check syntax around operations/blocks."
                    };
                    send_json(
                        &mut stream,
                        "200 OK",
                        json!({ "ok": true, "data": { "explanation": explanation } }),
                    );
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e), "error_code": "INVALID_PAYLOAD"}),
                ),
            }
        }
        ("POST", "/api/schemas") => {
            let req: Result<SchemasRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => {
                    let db = registry.get(&req.database);
                    match db {
                        Some(db) => match load_schemas(db) {
                            Ok(schemas) => send_json(
                                &mut stream,
                                "200 OK",
                                json!({"ok": true, "schemas": schemas}),
                            ),
                            Err(e) => send_json(
                                &mut stream,
                                "400 Bad Request",
                                json!({"ok": false, "error": e}),
                            ),
                        },
                        None => send_json(
                            &mut stream,
                            "404 Not Found",
                            json!({"ok": false, "error": "database not found"}),
                        ),
                    }
                }
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/schema_detail") => {
            let req: Result<SchemaDetailRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match load_schema_detail(db, &req.schema) {
                        Ok(schema) => {
                            send_json(&mut stream, "200 OK", json!({"ok": true, "schema": schema}))
                        }
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/documents") => {
            let req: Result<DocumentsRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => {
                        let page = req.page.unwrap_or(1);
                        let page_size = req.page_size.unwrap_or(20);
                        match load_documents(db, &req.schema, page, page_size) {
                            Ok(payload) => send_json(
                                &mut stream,
                                "200 OK",
                                json!({"ok": true, "page": payload}),
                            ),
                            Err(e) => send_json(
                                &mut stream,
                                "400 Bad Request",
                                json!({"ok": false, "error": e}),
                            ),
                        }
                    }
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/documents/update") => {
            let req: Result<UpdateDocumentRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match update_document(db, req) {
                        Ok(document) => send_json(
                            &mut stream,
                            "200 OK",
                            json!({"ok": true, "document": document}),
                        ),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/documents/delete") => {
            let req: Result<DeleteDocumentRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match delete_document(db, req) {
                        Ok(deleted) => {
                            if deleted {
                                send_json(
                                    &mut stream,
                                    "200 OK",
                                    json!({"ok": true, "deleted": true}),
                                )
                            } else {
                                send_json(
                                    &mut stream,
                                    "404 Not Found",
                                    json!({"ok": false, "deleted": false, "error": "document not found for delete"}),
                                )
                            }
                        }
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/documents/delete_all") => {
            let req: Result<DeleteAllDocumentsRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match delete_all_documents(db, req) {
                        Ok(deleted_count) => send_json(
                            &mut stream,
                            "200 OK",
                            json!({"ok": true, "deleted_count": deleted_count}),
                        ),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/documents/duplicate") => {
            let req: Result<DuplicateDocumentRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match duplicate_document(db, req) {
                        Ok(document) => send_json(
                            &mut stream,
                            "200 OK",
                            json!({"ok": true, "document": document}),
                        ),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/collections") => {
            let req: Result<CollectionsRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match load_collection_summaries(db) {
                        Ok(collections) => send_json(
                            &mut stream,
                            "200 OK",
                            json!({"ok": true, "collections": collections}),
                        ),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/collections/upsert") => {
            let req: Result<UpsertCollectionRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match upsert_collection_schema(db, req) {
                        Ok(schema) => {
                            send_json(&mut stream, "200 OK", json!({"ok": true, "schema": schema}))
                        }
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/collections/delete") => {
            let req: Result<DeleteCollectionRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match delete_collection(db, req) {
                        Ok(payload) => send_json(
                            &mut stream,
                            "200 OK",
                            json!({"ok": true, "result": payload}),
                        ),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/indexes/rebuild") => {
            let req: Result<RebuildIndexesRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match rebuild_indexes(db, req) {
                        Ok(payload) => send_json(&mut stream, "200 OK", payload),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/vector/indexes/list") => {
            let req: Result<ListVectorIndexesRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match list_vector_indexes(db, req) {
                        Ok(payload) => send_json(&mut stream, "200 OK", payload),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/vector/indexes/upsert") => {
            let req: Result<UpsertVectorIndexRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match upsert_vector_index(db, req) {
                        Ok(payload) => send_json(&mut stream, "200 OK", payload),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/vector/indexes/delete") => {
            let req: Result<DeleteVectorIndexRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match delete_vector_index(db, req) {
                        Ok(payload) => send_json(&mut stream, "200 OK", payload),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/vector/indexes/status") => {
            let req: Result<VectorIndexStatusRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match vector_index_status(db, req) {
                        Ok(payload) => send_json(&mut stream, "200 OK", payload),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/vector/indexes/build") => {
            let req: Result<BuildVectorIndexRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match build_vector_index(db, req) {
                        Ok(payload) => send_json(&mut stream, "200 OK", payload),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/vector/indexes/rebuild") => {
            let req: Result<BuildVectorIndexRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(mut req) => match registry.get(&req.database) {
                    Some(db) => {
                        req.force = Some(true);
                        match build_vector_index(db, req) {
                            Ok(payload) => send_json(&mut stream, "200 OK", payload),
                            Err(e) => send_json(
                                &mut stream,
                                "400 Bad Request",
                                json!({"ok": false, "error": e}),
                            ),
                        }
                    }
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/import") => {
            let req: Result<ImportDataRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match import_data(db, req) {
                        Ok(payload) => send_json(
                            &mut stream,
                            "200 OK",
                            json!({"ok": true, "result": payload}),
                        ),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        ("POST", "/api/seed_demo") => {
            let req: Result<SeedDemoRequest, _> = serde_json::from_str(&body);
            match req {
                Ok(req) => match registry.get(&req.database) {
                    Some(db) => match seed_feature_demo(db) {
                        Ok(payload) => send_json(&mut stream, "200 OK", payload),
                        Err(e) => send_json(
                            &mut stream,
                            "400 Bad Request",
                            json!({"ok": false, "error": e}),
                        ),
                    },
                    None => send_json(
                        &mut stream,
                        "404 Not Found",
                        json!({"ok": false, "error": "database not found"}),
                    ),
                },
                Err(e) => send_json(
                    &mut stream,
                    "400 Bad Request",
                    json!({"ok": false, "error": format!("invalid payload: {}", e)}),
                ),
            }
        }
        _ => {
            send_json(
                &mut stream,
                "404 Not Found",
                json!({"ok": false, "error": "route not found"}),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn test_required_role_matrix_for_core_routes() {
        assert_eq!(
            required_role_for_route("GET", "/api/databases"),
            Some(ApiRole::Readonly)
        );
        assert_eq!(
            required_role_for_route("POST", "/api/query"),
            Some(ApiRole::Readwrite)
        );
        assert_eq!(
            required_role_for_route("POST", "/api/documents/update"),
            Some(ApiRole::Readwrite)
        );
        assert_eq!(
            required_role_for_route("POST", "/api/collections/upsert"),
            Some(ApiRole::Admin)
        );
        assert_eq!(
            required_role_for_route("POST", "/api/databases/delete"),
            Some(ApiRole::Admin)
        );
        assert_eq!(required_role_for_route("GET", "/"), None);
    }

    #[test]
    fn test_required_role_matrix_for_all_api_families() {
        let readonly_routes = vec![
            ("GET", "/api/databases"),
            ("POST", "/api/schemas"),
            ("POST", "/api/schema_detail"),
            ("POST", "/api/documents"),
            ("POST", "/api/collections"),
            ("POST", "/api/metrics"),
            ("POST", "/api/cluster/topology"),
            ("POST", "/api/ai/chat"),
            ("POST", "/api/ai/generate_query"),
            ("POST", "/api/ai/explain_query"),
        ];
        for (method, path) in readonly_routes {
            assert_eq!(
                required_role_for_route(method, path),
                Some(ApiRole::Readonly),
                "expected readonly for {} {}",
                method,
                path
            );
        }

        let readwrite_routes = vec![
            ("POST", "/api/query"),
            ("POST", "/api/query/explain"),
            ("POST", "/api/documents/update"),
            ("POST", "/api/documents/delete"),
            ("POST", "/api/documents/delete_all"),
            ("POST", "/api/documents/duplicate"),
            ("POST", "/api/import"),
            ("POST", "/api/seed_demo"),
        ];
        for (method, path) in readwrite_routes {
            assert_eq!(
                required_role_for_route(method, path),
                Some(ApiRole::Readwrite),
                "expected readwrite for {} {}",
                method,
                path
            );
        }

        let admin_routes = vec![
            ("POST", "/api/databases"),
            ("POST", "/api/databases/delete"),
            ("POST", "/api/collections/upsert"),
            ("POST", "/api/collections/delete"),
            ("POST", "/api/indexes/rebuild"),
        ];
        for (method, path) in admin_routes {
            assert_eq!(
                required_role_for_route(method, path),
                Some(ApiRole::Admin),
                "expected admin for {} {}",
                method,
                path
            );
        }
    }

    #[test]
    fn test_role_allows_matrix() {
        assert!(role_allows(ApiRole::Admin, ApiRole::Admin));
        assert!(role_allows(ApiRole::Admin, ApiRole::Readwrite));
        assert!(role_allows(ApiRole::Admin, ApiRole::Readonly));

        assert!(!role_allows(ApiRole::Readwrite, ApiRole::Admin));
        assert!(role_allows(ApiRole::Readwrite, ApiRole::Readwrite));
        assert!(role_allows(ApiRole::Readwrite, ApiRole::Readonly));

        assert!(!role_allows(ApiRole::Readonly, ApiRole::Admin));
        assert!(!role_allows(ApiRole::Readonly, ApiRole::Readwrite));
        assert!(role_allows(ApiRole::Readonly, ApiRole::Readonly));
    }

    #[test]
    fn test_authenticated_role_from_bearer_token() {
        let _guard = env_lock().lock().expect("env lock");
        std::env::set_var("OWL_EXPLORER_READONLY_TOKEN", "ro");
        std::env::set_var("OWL_EXPLORER_READWRITE_TOKEN", "rw");
        std::env::set_var("OWL_EXPLORER_ADMIN_TOKEN", "admin");

        let mut headers = HashMap::new();
        headers.insert("authorization".to_string(), "Bearer rw".to_string());
        assert_eq!(authenticated_role(&headers), Some(ApiRole::Readwrite));

        headers.insert("authorization".to_string(), "Bearer admin".to_string());
        assert_eq!(authenticated_role(&headers), Some(ApiRole::Admin));

        headers.insert("authorization".to_string(), "Bearer ro".to_string());
        assert_eq!(authenticated_role(&headers), Some(ApiRole::Readonly));
    }

    #[test]
    fn test_authenticated_role_from_x_owl_token() {
        let _guard = env_lock().lock().expect("env lock");
        std::env::set_var("OWL_EXPLORER_READONLY_TOKEN", "ro");
        std::env::set_var("OWL_EXPLORER_READWRITE_TOKEN", "rw");
        std::env::set_var("OWL_EXPLORER_ADMIN_TOKEN", "admin");

        let mut headers = HashMap::new();
        headers.insert("x-owl-token".to_string(), "admin".to_string());
        assert_eq!(authenticated_role(&headers), Some(ApiRole::Admin));
    }
}
