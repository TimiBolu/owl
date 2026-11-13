mod bootstrap;
mod fingerprint;
mod ingest;
mod model;
mod query;
mod web;

use anyhow::{anyhow, Context, Result};
use chrono::{Duration, Utc};
use clap::{Parser, Subcommand};
use ingest::IngestService;
use model::{IngestEvent, StackFrame};
use owl_db::storage_engine::RustStore;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Parser)]
#[command(
    name = "loggr",
    version,
    about = "A local-first error investigation workbench powered by Owl"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the ingest API and investigation workbench.
    Serve {
        #[arg(long, default_value = "./loggr.owl")]
        db: PathBuf,
        #[arg(long, default_value = "127.0.0.1:7340")]
        bind: SocketAddr,
        #[arg(long, default_value_t = 4)]
        max_gib: usize,
    },
    /// Send a JSON event or event array to a running Loggr.
    Send {
        file: PathBuf,
        #[arg(long, default_value = "http://127.0.0.1:7340")]
        url: String,
    },
    /// Fill a running Loggr with realistic checkout failures and trace context.
    Demo {
        #[arg(long, default_value_t = 240)]
        count: usize,
        #[arg(long, default_value = "http://127.0.0.1:7340")]
        url: String,
    },
    /// Run Owl's offline integrity checker against a Loggr database.
    Check {
        #[arg(long, default_value = "./loggr.owl")]
        db: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    match Cli::parse().command {
        Command::Serve { db, bind, max_gib } => serve(db, bind, max_gib).await,
        Command::Send { file, url } => send_file(&file, &url),
        Command::Demo { count, url } => send_demo(count, &url),
        Command::Check { db } => check(&db),
    }
}

async fn serve(db: PathBuf, bind: SocketAddr, max_gib: usize) -> Result<()> {
    if !is_loopback(bind.ip()) {
        return Err(anyhow!("this first local release binds only to a loopback address; authenticated remote binding is not enabled yet"));
    }
    let max_size = max_gib
        .checked_mul(1024 * 1024 * 1024)
        .ok_or_else(|| anyhow!("max-gib is too large"))?;
    let storage_started = Instant::now();
    let store = Arc::new(
        RustStore::new(&db, max_size)
            .with_context(|| format!("open Owl database {}", db.display()))?,
    );
    let storage_elapsed = storage_started.elapsed();
    let bootstrap_started = Instant::now();
    bootstrap::bootstrap(&store)?;
    let bootstrap_elapsed = bootstrap_started.elapsed();
    let ingest = IngestService::start(store.clone())?;
    let app = web::router(store.clone(), ingest);
    let listener = tokio::net::TcpListener::bind(bind).await?;
    println!("loggr is ready at http://{bind}");
    println!("owl database: {}", db.display());
    println!(
        "startup: owl storage {:.2} ms · loggr bootstrap {:.2} ms",
        storage_elapsed.as_secs_f64() * 1_000.0,
        bootstrap_elapsed.as_secs_f64() * 1_000.0
    );
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown())
        .await?;
    store.wait_for_maintenance()?;
    Ok(())
}

async fn shutdown() {
    let _ = tokio::signal::ctrl_c().await;
}

fn is_loopback(ip: IpAddr) -> bool {
    ip.is_loopback()
}

fn check(path: &Path) -> Result<()> {
    let report = RustStore::check_integrity(path)?;
    println!("integrity: ok");
    println!(
        "durable LSN: {} · live objects: {} · scalar indexes: {}",
        report.durable_lsn, report.live_objects, report.scalar_indexes
    );
    println!(
        "checkpoint: {} · files checked: {}",
        report.checkpoint_sequence, report.files_checked
    );
    Ok(())
}

fn send_file(path: &Path, url: &str) -> Result<()> {
    let body = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_slice(&body).context("event file is not valid JSON")?;
    let endpoint = if value.is_array() {
        "/api/v1/events/batch"
    } else {
        "/api/v1/events"
    };
    let response = post_json(url, endpoint, &body)?;
    println!("{response}");
    Ok(())
}

fn send_demo(count: usize, url: &str) -> Result<()> {
    let mut sent = 0;
    while sent < count {
        let batch_len = (count - sent).min(256);
        let events = (sent..sent + batch_len).map(demo_event).collect::<Vec<_>>();
        let body = serde_json::to_vec(&events)?;
        post_json(url, "/api/v1/events/batch", &body)?;
        sent += batch_len;
        print!("\rseeded {sent}/{count} events");
        std::io::stdout().flush()?;
    }
    println!("\nopen {url} and investigate checkout-api / production");
    Ok(())
}

fn demo_event(index: usize) -> IngestEvent {
    let application = match index % 4 {
        0 => "checkout-api",
        1 => "inventory-worker",
        2 => "game-server",
        _ => "storefront-web",
    };
    let environment = match (index / 12) % 10 {
        0..=5 => "production",
        6..=7 => "staging",
        _ => "local",
    };
    let exception = index % 7 == 0 || index % 11 == 0;
    let warning = !exception && index % 5 == 0;
    let (level, kind, message, exception_type) = if exception {
        let (message, exception_type) = match application {
            "checkout-api" => (
                format!("Payment authorization timed out after {} ms", 1800 + index),
                "GatewayTimeout",
            ),
            "inventory-worker" => (
                format!("Reservation {} lost its optimistic lock", 42000 + index),
                "ReservationConflict",
            ),
            "game-server" => (
                format!(
                    "World tick {} exceeded its simulation budget",
                    700000 + index
                ),
                "TickBudgetExceeded",
            ),
            _ => (
                format!("Product route {} failed during server render", 9000 + index),
                "RenderFailure",
            ),
        };
        (
            "error",
            "exception",
            message,
            Some(exception_type.to_string()),
        )
    } else if warning {
        (
            "warn",
            "log",
            format!("{application} operation took {} ms", 500 + index),
            None,
        )
    } else if index % 8 == 0 {
        (
            "debug",
            "log",
            format!(
                "{application} cache decision recorded for key {}",
                81000 + index
            ),
            None,
        )
    } else if index % 13 == 0 {
        (
            "trace",
            "log",
            format!("{application} entered pipeline stage {}", index % 9),
            None,
        )
    } else {
        (
            "info",
            "log",
            format!("{application} request {} completed", 800000 + index),
            None,
        )
    };
    IngestEvent {
        id: Some(format!("demo-v2-{index:08}")),
        application: application.into(),
        environment: environment.into(),
        occurred_at: Some(Utc::now() - Duration::seconds((index % 1800) as i64)),
        level: level.into(),
        kind: kind.into(),
        message,
        exception_type,
        fingerprint: None,
        trace_id: Some(format!("01JTRACE{:08X}", index / 3)),
        span_id: Some(format!("span-{index:08x}")),
        release: Some(
            if index % 3 == 0 {
                "2026.07.23+9f31c2"
            } else {
                "2026.07.22+7aa19d"
            }
            .into(),
        ),
        stack: if exception {
            demo_stack(application)
        } else {
            Vec::new()
        },
        attributes: HashMap::from([
            (
                "request_id".into(),
                serde_json::json!(format!("req-{index:08}")),
            ),
            ("region".into(), serde_json::json!("eu-west-1")),
            ("duration_ms".into(), serde_json::json!(41 + index % 2200)),
        ]),
    }
}

fn demo_stack(application: &str) -> Vec<StackFrame> {
    let (module, function, file) = match application {
        "checkout-api" => ("checkout::payment", "authorize_payment", "src/payment.rs"),
        "inventory-worker" => (
            "inventory::reservation",
            "reserve_stock",
            "src/reservation.rs",
        ),
        "game-server" => ("world::simulation", "advance_world", "src/world/tick.rs"),
        _ => (
            "storefront::render",
            "render_product",
            "src/routes/product.rs",
        ),
    };
    vec![
        StackFrame {
            module: Some(module.into()),
            function: function.into(),
            file: file.into(),
            line: Some(184),
            column: Some(17),
            in_app: true,
        },
        StackFrame {
            module: Some("checkout::flow".into()),
            function: "complete_checkout".into(),
            file: "src/checkout.rs".into(),
            line: Some(92),
            column: Some(11),
            in_app: true,
        },
        StackFrame {
            module: Some("axum::handler".into()),
            function: "Handler::call".into(),
            file: "/cargo/registry/axum/src/handler.rs".into(),
            line: Some(241),
            column: None,
            in_app: false,
        },
        StackFrame {
            module: Some("tokio::runtime".into()),
            function: "task::poll".into(),
            file: "/cargo/registry/tokio/src/runtime/task.rs".into(),
            line: Some(449),
            column: None,
            in_app: false,
        },
    ]
}

fn post_json(base_url: &str, path: &str, body: &[u8]) -> Result<String> {
    let authority = base_url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("only http:// URLs are supported by the compact CLI client"))?
        .trim_end_matches('/');
    let mut stream =
        TcpStream::connect(authority).with_context(|| format!("connect to {authority}"))?;
    stream.write_all(format!("POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).as_bytes())?;
    stream.write_all(body)?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .ok_or_else(|| anyhow!("invalid HTTP response"))?;
    if !head
        .lines()
        .next()
        .is_some_and(|line| line.contains(" 200 ") || line.contains(" 202 "))
    {
        return Err(anyhow!("Loggr rejected the request: {body}"));
    }
    Ok(body.to_string())
}
