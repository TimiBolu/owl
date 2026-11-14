use anyhow::{anyhow, Context, Result};
use clap::Parser;
use owl_db::storage::RustStore;
use owl_server::{router, Options};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

/// Serve an Owl database over HTTP.
#[derive(Parser)]
#[command(name = "owl-server", version)]
struct Arguments {
    /// The database file. Created if it does not exist.
    #[arg(long, env = "OWL_DATA", default_value = "owl.db")]
    data: PathBuf,
    /// The address to listen on.
    #[arg(long, env = "OWL_LISTEN", default_value = "127.0.0.1:7878")]
    listen: SocketAddr,
    /// Require this bearer token on every request but the health check.
    #[arg(long, env = "OWL_TOKEN", hide_env_values = true)]
    token: Option<String>,
    /// The most the database may grow to, in GiB.
    #[arg(long, default_value_t = 64)]
    max_gib: usize,
    /// Roll back a transaction left untouched for this many seconds.
    #[arg(long, default_value_t = 60)]
    transaction_idle_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<()> {
    let arguments = Arguments::parse();
    if arguments.token.is_none() && !arguments.listen.ip().is_loopback() {
        return Err(anyhow!(
            "refusing to listen on {} without --token: anyone who can reach it could read and \
             write the database",
            arguments.listen
        ));
    }
    let max_size = arguments
        .max_gib
        .checked_mul(1 << 30)
        .ok_or_else(|| anyhow!("--max-gib is too large"))?;
    let store = Arc::new(
        RustStore::new(&arguments.data, max_size)
            .with_context(|| format!("open Owl database {}", arguments.data.display()))?,
    );
    let (app, janitor) = router(
        store.clone(),
        Options {
            token: arguments.token,
            transaction_idle: Duration::from_secs(arguments.transaction_idle_seconds.max(1)),
        },
    );
    tokio::spawn(janitor.run());
    let listener = tokio::net::TcpListener::bind(arguments.listen)
        .await
        .with_context(|| format!("listen on {}", arguments.listen))?;
    println!(
        "owl-server is ready at http://{} serving {}",
        listener.local_addr()?,
        arguments.data.display()
    );
    axum::serve(listener, app)
        // Answers are small; holding one back to batch it only adds latency.
        .tcp_nodelay(true)
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    store.wait_for_maintenance()?;
    Ok(())
}
