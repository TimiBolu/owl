use anyhow::{anyhow, Context, Result};
use clap::Parser;
use owl_db::storage::RustStore;
use owl_db::storage_engine::S3Config;
use owl_server::{
    checkpoint_if_written, open_in_objects, router, ObjectOptions, Objects, Options, Reader, Role,
};
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
    /// Hold at most this many documents decoded in memory. Rows beyond it
    /// are kept as their records and decoded when read: less memory,
    /// slower scans. Without it every row stays decoded.
    #[arg(long, env = "OWL_DECODED_DOCUMENTS")]
    decoded_documents: Option<usize>,
    /// Where `POST /v1/backups` writes backups. Without it none are taken.
    #[arg(long, env = "OWL_BACKUP_DIR")]
    backup_dir: Option<PathBuf>,
    /// Before serving, restore this backup as the database at --data,
    /// which must not exist yet.
    #[arg(long)]
    restore_from: Option<PathBuf>,
    /// Roll back a transaction left untouched for this many seconds.
    #[arg(long, default_value_t = 60)]
    transaction_idle_seconds: u64,

    /// Keep the database's parts in this S3 bucket as well as on disk.
    /// The access key and secret are read from AWS_ACCESS_KEY_ID and
    /// AWS_SECRET_ACCESS_KEY.
    #[arg(long, env = "OWL_S3_BUCKET")]
    s3_bucket: Option<String>,
    /// The bucket's region.
    #[arg(long, env = "OWL_S3_REGION", default_value = "us-east-1")]
    s3_region: String,
    /// The host to reach the bucket at, with its port if it has one, for
    /// a server that speaks S3 and is not AWS. The bucket then goes in
    /// the path.
    #[arg(long, env = "OWL_S3_ENDPOINT")]
    s3_endpoint: Option<String>,
    /// Reach --s3-endpoint over plain HTTP, not TLS.
    #[arg(long)]
    s3_insecure: bool,
    /// What every key begins with in the bucket.
    #[arg(long, env = "OWL_S3_PREFIX", default_value = "")]
    s3_prefix: String,
    #[arg(long, env = "AWS_ACCESS_KEY_ID", hide = true, hide_env_values = true)]
    s3_access_key: Option<String>,
    #[arg(long, env = "AWS_SECRET_ACCESS_KEY", hide = true, hide_env_values = true)]
    s3_secret_key: Option<String>,
    /// Keep the database's parts in this directory, one file an object,
    /// as an object store. For a mounted bucket, or to try it out.
    #[arg(long, env = "OWL_OBJECTS_DIR", conflicts_with = "s3_bucket")]
    objects_dir: Option<PathBuf>,
    /// With an object store: serve what was last published there, taking
    /// no writes. --data is where this server keeps its copy.
    #[arg(long)]
    read_only: bool,
    /// With an object store: become the database's writer, starting from
    /// what was last published, in place of the server that was. Any
    /// files of the database at --data are replaced.
    #[arg(long, conflicts_with = "read_only")]
    take_over: bool,
    /// The most the local cache of parts read from the object store may
    /// hold, in MiB.
    #[arg(long, default_value_t = 256)]
    object_cache_mib: u64,
    /// Make a checkpoint this often, in seconds, when anything has been
    /// written since the last. With an object store a checkpoint is when
    /// readers see new data, so this bounds how far behind they can be.
    /// Without it checkpoints are made by size alone.
    #[arg(long, env = "OWL_CHECKPOINT_SECONDS")]
    checkpoint_seconds: Option<u64>,
    /// With --read-only: look for a newer publication this often, in
    /// seconds, and serve it when there is one. Without it the server
    /// shows what was published when it started.
    #[arg(long, env = "OWL_REFRESH_SECONDS", requires = "read_only")]
    refresh_seconds: Option<u64>,
}

impl Arguments {
    /// The object store asked for, if one was.
    fn objects(&self) -> Result<Option<ObjectOptions>> {
        let objects = match (&self.s3_bucket, &self.objects_dir) {
            (Some(bucket), _) => {
                let key = |value: &Option<String>, name: &str| {
                    value.clone().ok_or_else(|| anyhow!("--s3-bucket needs {name} in the environment"))
                };
                let mut config = S3Config::aws(
                    bucket,
                    &self.s3_region,
                    &key(&self.s3_access_key, "AWS_ACCESS_KEY_ID")?,
                    &key(&self.s3_secret_key, "AWS_SECRET_ACCESS_KEY")?,
                );
                config.prefix = self.s3_prefix.clone();
                if let Some(endpoint) = &self.s3_endpoint {
                    config.endpoint = endpoint.clone();
                    config.path_style = true;
                }
                if self.s3_insecure && self.s3_endpoint.is_none() {
                    return Err(anyhow!("--s3-insecure is for --s3-endpoint; AWS is reached over TLS"));
                }
                Objects::S3 {
                    config,
                    secure: !self.s3_insecure,
                }
            }
            (None, Some(directory)) => Objects::Directory(directory.clone()),
            (None, None) => {
                if self.read_only || self.take_over {
                    return Err(anyhow!("--read-only and --take-over need --s3-bucket or --objects-dir"));
                }
                return Ok(None);
            }
        };
        Ok(Some(ObjectOptions {
            objects,
            role: match (self.read_only, self.take_over) {
                (true, _) => Role::Reader,
                (_, true) => Role::TakeOver,
                _ => Role::Writer,
            },
            cache_bytes: self.object_cache_mib << 20,
        }))
    }
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
    let objects = arguments.objects()?;
    if objects.is_some() && arguments.restore_from.is_some() {
        return Err(anyhow!("--restore-from is not for a database kept in an object store"));
    }
    let mut reader = None;
    let store = match (&objects, &arguments.restore_from) {
        (Some(objects), _) => {
            let store = match objects.role {
                Role::Reader => {
                    let (opened, store) =
                        Reader::open(&arguments.data, max_size, objects).map_err(|error| anyhow!(error))?;
                    reader = Some(opened);
                    store
                }
                _ => open_in_objects(&arguments.data, max_size, objects).map_err(|error| anyhow!(error))?,
            };
            println!(
                "{} {} at commit {}, with its parts in an object store",
                match objects.role {
                    Role::Writer => "writing",
                    Role::TakeOver => "took over",
                    Role::Reader => "reading a publication of",
                },
                arguments.data.display(),
                store.committed_lsn()
            );
            store
        }
        (None, Some(backup)) => {
            let store = RustStore::restore(backup, &arguments.data, max_size)
                .with_context(|| format!("restore {}", backup.display()))?;
            println!(
                "restored {} as {} at commit {}",
                backup.display(),
                arguments.data.display(),
                store.committed_lsn()
            );
            Arc::new(store)
        }
        (None, None) => Arc::new(
            RustStore::new(&arguments.data, max_size)
                .with_context(|| format!("open Owl database {}", arguments.data.display()))?,
        ),
    };
    let (app, janitor) = router(
        store.clone(),
        Options {
            token: arguments.token,
            transaction_idle: Duration::from_secs(arguments.transaction_idle_seconds.max(1)),
            backup_directory: arguments.backup_dir,
        },
    );
    let slot = janitor.store();
    tokio::spawn(janitor.run());
    if let Some(seconds) = arguments.checkpoint_seconds.filter(|_| reader.is_none()) {
        let slot = slot.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(seconds.max(1)));
            tick.tick().await;
            loop {
                tick.tick().await;
                let store = slot.get();
                // A checkpoint is disk work, and must not hold up requests.
                match tokio::task::spawn_blocking(move || checkpoint_if_written(&store)).await {
                    Ok(Err(error)) => eprintln!("timed checkpoint failed: {error}"),
                    Ok(Ok(_)) | Err(_) => {}
                }
            }
        });
    }
    if let (Some(seconds), Some(mut reader)) = (arguments.refresh_seconds, reader) {
        let decoded = arguments.decoded_documents;
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(seconds.max(1)));
            tick.tick().await;
            loop {
                tick.tick().await;
                let refreshed = tokio::task::spawn_blocking(move || {
                    let outcome = reader.refresh();
                    (reader, outcome)
                })
                .await;
                let Ok((returned, outcome)) = refreshed else { break };
                reader = returned;
                match outcome {
                    Ok(Some(store)) => {
                        if let Some(budget) = decoded {
                            let _ = store.set_decoded_budget(Some(budget));
                        }
                        println!("now serving publication {}", reader.published());
                        slot.replace(store);
                    }
                    Ok(None) => {}
                    // The publication being served is still served.
                    Err(error) => eprintln!("refresh failed: {error}"),
                }
            }
        });
    }
    let listener = tokio::net::TcpListener::bind(arguments.listen)
        .await
        .with_context(|| format!("listen on {}", arguments.listen))?;
    if let Some(budget) = arguments.decoded_documents {
        store
            .set_decoded_budget(Some(budget))
            .context("apply --decoded-documents")?;
    }
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
