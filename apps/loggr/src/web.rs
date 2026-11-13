use crate::ingest::{IngestService, SubmitError};
use crate::model::IngestEvent;
use crate::query::{self, EventFilter, IssueFilter};
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use owl_db::storage_engine::RustStore;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
struct AppState {
    store: Arc<RustStore>,
    ingest: IngestService,
}

pub fn router(store: Arc<RustStore>, ingest: IngestService) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/loggr.css", get(css))
        .route("/app.js", get(js))
        .route("/api/v1/health", get(health))
        .route("/api/v1/query", post(execute_oql))
        .route("/api/v1/events", post(ingest_one).get(list_events))
        .route("/api/v1/events/batch", post(ingest_batch))
        .route("/api/v1/events/:id", get(event_detail))
        .route("/api/v1/traces/:trace_id", get(trace_events))
        .route("/api/v1/issues", get(list_issues))
        .route("/api/v1/issues/:fingerprint", get(issue_detail))
        .layer(DefaultBodyLimit::max(8 * 1024 * 1024))
        .with_state(AppState { store, ingest })
}

async fn index() -> Html<&'static str> {
    Html(include_str!("../web/index.html"))
}
async fn css() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        include_str!("../web/loggr.css"),
    )
}
async fn js() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "text/javascript; charset=utf-8")],
        include_str!("../web/app.js"),
    )
}
async fn health() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok", "storage": "owl", "mode": "single-node" }))
}

async fn execute_oql(
    State(state): State<AppState>,
    Json(request): Json<query::OqlRequest>,
) -> Response {
    let store = state.store;
    match tokio::task::spawn_blocking(move || query::execute_oql(store, request.query)).await {
        Ok(Ok(result)) => ([(CONTENT_TYPE, "application/json")], result).into_response(),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(error) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
    }
}

async fn ingest_one(State(state): State<AppState>, body: Bytes) -> Response {
    if body.len() > 1024 * 1024 {
        return api_error(StatusCode::PAYLOAD_TOO_LARGE, "event exceeds 1 MiB");
    }
    let event = match serde_json::from_slice::<IngestEvent>(&body) {
        Ok(event) => event,
        Err(error) => {
            return api_error(StatusCode::BAD_REQUEST, &format!("invalid event: {error}"))
        }
    };
    match state.ingest.submit(vec![event]).await {
        Ok(mut results) => (StatusCode::ACCEPTED, Json(results.remove(0))).into_response(),
        Err(error) => submit_error(error),
    }
}

async fn ingest_batch(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let ndjson = headers
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.starts_with("application/x-ndjson"));
    let events = if ndjson {
        let mut events = Vec::new();
        for (line, bytes) in body
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.iter().all(u8::is_ascii_whitespace))
            .enumerate()
        {
            match serde_json::from_slice(bytes) {
                Ok(event) => events.push(event),
                Err(error) => {
                    return api_error(
                        StatusCode::BAD_REQUEST,
                        &format!("invalid NDJSON line {}: {error}", line + 1),
                    )
                }
            }
        }
        events
    } else {
        match serde_json::from_slice::<Vec<IngestEvent>>(&body) {
            Ok(events) => events,
            Err(error) => {
                return api_error(
                    StatusCode::BAD_REQUEST,
                    &format!("invalid event batch: {error}"),
                )
            }
        }
    };
    match state.ingest.submit(events).await {
        Ok(results) => (StatusCode::ACCEPTED, Json(results)).into_response(),
        Err(error) => submit_error(error),
    }
}

async fn list_events(State(state): State<AppState>, Query(filter): Query<EventFilter>) -> Response {
    let store = state.store;
    match tokio::task::spawn_blocking(move || query::events(&store, filter)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(error) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
    }
}

async fn list_issues(State(state): State<AppState>, Query(filter): Query<IssueFilter>) -> Response {
    let store = state.store;
    match tokio::task::spawn_blocking(move || query::issues(&store, filter)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(error) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
    }
}

async fn event_detail(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let store = state.store;
    match tokio::task::spawn_blocking(move || query::event_detail(&store, &id)).await {
        Ok(Ok(Some(document))) => Json(document).into_response(),
        Ok(Ok(None)) => api_error(StatusCode::NOT_FOUND, "event not found"),
        Ok(Err(error)) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
        Err(error) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
    }
}

async fn trace_events(State(state): State<AppState>, Path(trace_id): Path<String>) -> Response {
    let store = state.store;
    let filter = EventFilter {
        application: None,
        environment: None,
        level: None,
        fingerprint: None,
        trace_id: Some(trace_id),
        text: None,
        limit: Some(200),
        cursor: None,
    };
    match tokio::task::spawn_blocking(move || query::events(&store, filter)).await {
        Ok(Ok(result)) => Json(result).into_response(),
        Ok(Err(error)) => api_error(StatusCode::BAD_REQUEST, &error.to_string()),
        Err(error) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
    }
}

async fn issue_detail(State(state): State<AppState>, Path(fingerprint): Path<String>) -> Response {
    let store = state.store;
    match tokio::task::spawn_blocking(move || query::issue_detail(&store, &fingerprint)).await {
        Ok(Ok(Some(document))) => Json(document).into_response(),
        Ok(Ok(None)) => api_error(StatusCode::NOT_FOUND, "issue not found"),
        Ok(Err(error)) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
        Err(error) => api_error(StatusCode::SERVICE_UNAVAILABLE, &error.to_string()),
    }
}

fn submit_error(error: SubmitError) -> Response {
    match error {
        SubmitError::Invalid(message) => api_error(StatusCode::BAD_REQUEST, &message),
        SubmitError::Saturated => (
            StatusCode::TOO_MANY_REQUESTS,
            [(header::RETRY_AFTER, "1")],
            Json(json!({ "error": "ingest queue is full" })),
        )
            .into_response(),
        SubmitError::Unavailable(message) => api_error(StatusCode::SERVICE_UNAVAILABLE, &message),
    }
}

fn api_error(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({ "error": message }))).into_response()
}
