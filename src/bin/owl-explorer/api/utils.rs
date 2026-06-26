use crate::api::models::Registry;
use chrono::Utc;
use serde_json::json;
use std::io::Write;
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub static RESPONSE_COUNTER: AtomicU64 = AtomicU64::new(1);

pub fn project_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

pub fn resolve_db_path(path: &str) -> PathBuf {
    let p = PathBuf::from(path);
    if p.is_absolute() {
        p
    } else {
        project_root().join(p)
    }
}

pub fn db_size_bytes(size_mb: u64) -> usize {
    (size_mb.max(1) as usize) * 1024 * 1024
}

pub fn normalize_api_payload(status: &str, body: serde_json::Value) -> serde_json::Value {
    let mut payload = match body {
        serde_json::Value::Object(map) => serde_json::Value::Object(map),
        other => json!({ "ok": status.starts_with("200"), "data": other }),
    };
    let response_id = RESPONSE_COUNTER.fetch_add(1, Ordering::Relaxed);
    if let Some(map) = payload.as_object_mut() {
        let inferred_ok = status.starts_with("200");
        if !map.contains_key("ok") {
            map.insert("ok".to_string(), json!(inferred_ok));
        }
        if !map.contains_key("api_version") {
            map.insert("api_version".to_string(), json!("v1"));
        }
        if !map.contains_key("request_id") {
            map.insert(
                "request_id".to_string(),
                json!(format!("owl-{}", response_id)),
            );
        }
        if !map.contains_key("timestamp") {
            map.insert("timestamp".to_string(), json!(Utc::now().to_rfc3339()));
        }

        let ok = map.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        if !ok && !map.contains_key("error_code") {
            let code = if status.starts_with("404") {
                "NOT_FOUND"
            } else if status.starts_with("401") {
                "AUTH_REQUIRED"
            } else if status.starts_with("403") {
                "FORBIDDEN"
            } else if status.starts_with("400") {
                "BAD_REQUEST"
            } else if status.starts_with("500") {
                "INTERNAL_ERROR"
            } else {
                "UNKNOWN_ERROR"
            };
            map.insert("error_code".to_string(), json!(code));
        }
        if !ok {
            if !map.contains_key("message") {
                let message = map
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("request failed")
                    .to_string();
                map.insert("message".to_string(), json!(message));
            }
            if !map.contains_key("error") {
                map.insert("error".to_string(), json!("request failed"));
            }
            if !map.contains_key("details") {
                map.insert("details".to_string(), json!(null));
            }
        }
    }
    payload
}

pub fn send_json(stream: &mut TcpStream, status: &str, body: serde_json::Value) {
    let normalized = normalize_api_payload(status, body);
    let effective_status = effective_json_status(status, &normalized);
    let payload = normalized.to_string();
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        effective_status,
        payload.len(),
        payload
    );
    let _ = stream.write_all(response.as_bytes());
}

fn effective_json_status<'a>(status: &'a str, normalized_payload: &serde_json::Value) -> &'a str {
    if status.starts_with("200")
        && normalized_payload
            .get("ok")
            .and_then(|v| v.as_bool())
            .map(|ok| !ok)
            .unwrap_or(false)
    {
        "400 Bad Request"
    } else {
        status
    }
}

pub fn send_html(stream: &mut TcpStream, status: &str, body: &str) {
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        body.len(),
        body
    );
    let _ = stream.write_all(response.as_bytes());
}

pub fn send_bytes(stream: &mut TcpStream, status: &str, content_type: &str, body: &[u8]) {
    let headers = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        status,
        content_type,
        body.len()
    );
    let _ = stream.write_all(headers.as_bytes());
}

pub fn content_type_for_path(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
    {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "application/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

pub fn collect_storage_size_bytes(registry: &Registry, target_db: Option<&str>) -> u64 {
    registry
        .databases
        .iter()
        .filter(|db| target_db.map(|t| t == db.name).unwrap_or(true))
        .map(|db| resolve_db_path(&db.path))
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|m| m.len())
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_api_payload_error_contract_fields_present() {
        let payload = normalize_api_payload(
            "400 Bad Request",
            json!({"ok": false, "error": "invalid payload"}),
        );
        assert_eq!(payload.get("ok").and_then(|v| v.as_bool()), Some(false));
        assert!(payload.get("error_code").is_some());
        assert!(payload.get("message").is_some());
        assert!(payload.get("details").is_some());
        assert!(payload.get("request_id").is_some());
        assert!(payload.get("timestamp").is_some());
        assert!(payload.get("api_version").is_some());
    }

    #[test]
    fn test_effective_json_status_converts_ok_false_from_200() {
        let payload = json!({"ok": false, "error": "boom"});
        let status = effective_json_status("200 OK", &payload);
        assert_eq!(status, "400 Bad Request");
    }

    #[test]
    fn test_normalize_api_payload_infers_error_code_by_status() {
        let p401 = normalize_api_payload("401 Unauthorized", json!({"ok": false, "error": "auth"}));
        assert_eq!(
            p401.get("error_code").and_then(|v| v.as_str()),
            Some("AUTH_REQUIRED")
        );

        let p403 =
            normalize_api_payload("403 Forbidden", json!({"ok": false, "error": "forbidden"}));
        assert_eq!(
            p403.get("error_code").and_then(|v| v.as_str()),
            Some("FORBIDDEN")
        );

        let p404 = normalize_api_payload("404 Not Found", json!({"ok": false, "error": "missing"}));
        assert_eq!(
            p404.get("error_code").and_then(|v| v.as_str()),
            Some("NOT_FOUND")
        );
    }

    #[test]
    fn test_effective_json_status_keeps_non_200_error_status() {
        let payload = json!({"ok": false, "error": "denied"});
        assert_eq!(
            effective_json_status("403 Forbidden", &payload),
            "403 Forbidden"
        );
    }

    // Covered at helper-level to avoid network binding in sandboxed test runners.
}
