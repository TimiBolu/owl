//! Owl as a network service.
//!
//! One process owns the database file and answers JSON over HTTP:
//!
//! | Request | Does |
//! |---|---|
//! | `GET /v1/health` | Says the server is up. Needs no token. |
//! | `POST /v1/query` | Runs one OQL statement, with `$name` parameters. |
//! | `POST /v1/insert` | Inserts documents given as JSON, in one commit. |
//! | `POST /v1/script` | Runs an OQL script: schemas, functions, statements. |
//! | `POST /v1/transactions` | Opens a transaction and returns its id. |
//! | `POST /v1/transactions/{id}/commit` | Commits it, or answers 409. |
//! | `POST /v1/transactions/{id}/rollback` | Abandons it. |
//! | `POST /v1/backups` | Takes a backup into the server's backup directory. |
//!
//! `query` and `insert` take an optional `"transaction": id` and are then
//! part of that transaction. A statement outside a transaction is its own
//! commit and is atomic by itself. A commit answered with 409 and the code
//! `conflict` applied nothing: another commit changed a document the
//! transaction wrote, and the client runs the transaction again.
//!
//! Every failure is `{"error": {"code": .., "message": ..}}`.

use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use owl_db::error::OQLError;
use owl_db::storage::RustStore;
use owl_db::{bind_parameters, execute_oql_file, parse_oql, parse_oql_file, QueryExecutor, Value};
use serde::Deserialize;
use serde_json::{json, Map, Value as Json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Where a database's parts are kept besides the server's own disk.
#[derive(Clone, Debug)]
pub enum Objects {
    /// A directory: one file for each object. For a bucket that is
    /// mounted, and for trying things out.
    Directory(std::path::PathBuf),
    /// An S3 bucket, or one on a server that speaks S3. `secure` is
    /// whether to use TLS.
    S3 { config: owl_db::storage_engine::S3Config, secure: bool },
}

/// What this server is to a database kept in an object store.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Its one writer. It claims the database, and publishes after every
    /// checkpoint so that readers can follow. Refused if another writer
    /// has claimed the database since this directory last wrote it.
    Writer,
    /// Its writer from now on, starting from what was last published,
    /// in place of whichever server was writing it.
    TakeOver,
    /// A reader. It fetches the last publication and serves it. It takes
    /// no writes, and shows what was published when it started.
    Reader,
}

/// A database kept in an object store, and how to open it.
#[derive(Clone, Debug)]
pub struct ObjectOptions {
    pub objects: Objects,
    pub role: Role,
    /// The most the local cache of parts read from the object store may
    /// hold, in bytes.
    pub cache_bytes: u64,
}

fn object_store(objects: &Objects) -> Result<Arc<dyn owl_db::storage_engine::ObjectStore>, String> {
    use owl_db::storage_engine::{DirectoryObjectStore, HttpTransport, S3ObjectStore};
    Ok(match objects {
        Objects::Directory(path) => Arc::new(
            DirectoryObjectStore::new(path).map_err(|error| format!("open {}: {error}", path.display()))?,
        ),
        Objects::S3 { config, secure } => {
            let transport = Arc::new(HttpTransport::new(*secure, Duration::from_secs(60)));
            Arc::new(S3ObjectStore::over(config.clone(), transport))
        }
    })
}

/// The directory a database path is in and the name of its file, with the
/// directory named the way the store will name it.
fn directory_and_name(data: &std::path::Path) -> Result<(std::path::PathBuf, String), String> {
    let data = std::path::absolute(data).map_err(|error| format!("{}: {error}", data.display()))?;
    let directory = data.parent().ok_or("the database path has no directory")?.to_path_buf();
    let name = data
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("the database path has no file name")?
        .to_string();
    Ok((directory, name))
}

/// Open the database at `data` with its parts in an object store.
///
/// `data` is where the database's own files are kept on this machine: a
/// writer's log and roots, and the cache of parts it reads. A reader
/// keeps its copies of publications in a directory beside it; see
/// [`Reader`], which this makes and drops for a reader that will not be
/// refreshed.
pub fn open_in_objects(
    data: &std::path::Path,
    max_size_bytes: usize,
    options: &ObjectOptions,
) -> Result<Arc<RustStore>, String> {
    use owl_db::simulation::filesystem::RealFileSystem;
    use owl_db::storage_engine::ObjectFileSystem;
    if options.role == Role::Reader {
        return Ok(Reader::open(data, max_size_bytes, options)?.1);
    }
    let (directory, name) = directory_and_name(data)?;
    let objects = object_store(&options.objects)?;
    let local = Arc::new(RealFileSystem::new());
    let failed = |what: &str, error: std::io::Error| format!("{what} '{name}': {error}");
    let filesystem = match options.role {
        Role::TakeOver => ObjectFileSystem::take_over(local, objects, &directory, &name, options.cache_bytes)
            .map_err(|error| failed("take over", error))?,
        _ => ObjectFileSystem::writer(local, objects, &directory, &name, options.cache_bytes)
            .map_err(|error| failed("claim", error))?,
    };
    let store = RustStore::new_with_fs(directory.join(&name), max_size_bytes, filesystem.clone())
        .map_err(|error| format!("open {}: {error}", data.display()))?;
    store.publish_after_checkpoints(filesystem).map_err(|error| error.to_string())?;
    Ok(Arc::new(store))
}

/// A reader of a database in an object store, which can look for a newer
/// publication and open it.
///
/// Each publication it opens is fetched into a directory of its own,
/// under `<data>.copies`, so that the one being served is not touched
/// while the next is fetched. A copy is removed once the one after it has
/// been replaced in turn, by which time no request is still reading it.
pub struct Reader {
    objects: Arc<dyn owl_db::storage_engine::ObjectStore>,
    copies: std::path::PathBuf,
    name: String,
    max_size_bytes: usize,
    /// The parts read so far, kept across copies: a newer publication
    /// mostly lists parts an older one already fetched.
    cache: owl_db::storage_engine::PartCache,
    /// The number of the publication being served.
    published: u64,
    /// How many copies have been made.
    made: u64,
}

impl Reader {
    /// Fetch the last publication and open it.
    pub fn open(
        data: &std::path::Path,
        max_size_bytes: usize,
        options: &ObjectOptions,
    ) -> Result<(Self, Arc<RustStore>), String> {
        let (directory, name) = directory_and_name(data)?;
        let copies = directory.join(format!("{name}.copies"));
        // Copies an earlier run left are of no use to this one.
        if copies.exists() {
            std::fs::remove_dir_all(&copies).map_err(|error| format!("clear {}: {error}", copies.display()))?;
        }
        let cache = owl_db::storage_engine::PartCache::new(
            Arc::new(owl_db::simulation::filesystem::RealFileSystem::new()),
            copies.join("cache"),
            options.cache_bytes,
        )
        .map_err(|error| format!("make {}: {error}", copies.display()))?;
        let mut reader = Self {
            objects: object_store(&options.objects)?,
            copies,
            name,
            max_size_bytes,
            cache,
            published: 0,
            made: 0,
        };
        let store = reader.fetch()?;
        Ok((reader, store))
    }

    fn fetch(&mut self) -> Result<Arc<RustStore>, String> {
        use owl_db::simulation::filesystem::RealFileSystem;
        use owl_db::storage_engine::ObjectFileSystem;
        let directory = self.copies.join(self.made.to_string());
        let (filesystem, published) = ObjectFileSystem::reader_sharing(
            Arc::new(RealFileSystem::new()),
            self.objects.clone(),
            &directory,
            &self.name,
            &self.cache,
        )
        .map_err(|error| format!("fetch the publication of '{}': {error}", self.name))?;
        let store = RustStore::new_with_fs(directory.join(&self.name), self.max_size_bytes, filesystem)
            .map_err(|error| format!("open the publication of '{}': {error}", self.name))?;
        store.set_read_only(true);
        // The copy before the one this replaces has had a whole turn for
        // its requests to finish.
        if self.made >= 2 {
            let _ = std::fs::remove_dir_all(self.copies.join((self.made - 2).to_string()));
        }
        self.made += 1;
        self.published = published.number;
        Ok(Arc::new(store))
    }

    /// The number of the publication being served.
    pub fn published(&self) -> u64 {
        self.published
    }

    /// If a newer publication has been made, fetch and open it.
    pub fn refresh(&mut self) -> Result<Option<Arc<RustStore>>, String> {
        let newest = owl_db::storage_engine::Published::newest(self.objects.as_ref(), &self.name)
            .map_err(|error| format!("look for a publication of '{}': {error}", self.name))?;
        match newest {
            Some(newest) if newest.number != self.published => self.fetch().map(Some),
            _ => Ok(None),
        }
    }
}

/// Make a checkpoint if anything has been committed since the last one.
/// Says whether one was made.
///
/// A server that keeps its database in an object store publishes at each
/// checkpoint, so this is what bounds how far behind its readers can be.
pub fn checkpoint_if_written(store: &RustStore) -> Result<bool, String> {
    let status = store.maintenance_status().map_err(|error| error.to_string())?;
    if status.commits_since_checkpoint == 0 {
        return Ok(false);
    }
    store.checkpoint().map_err(|error| error.to_string())?;
    Ok(true)
}

/// How the server is run.
#[derive(Clone, Debug)]
pub struct Options {
    /// When set, every request but the health check must carry it as
    /// `Authorization: Bearer <token>`.
    pub token: Option<String>,
    /// A transaction no statement has touched for this long is rolled back,
    /// so a client that went away does not hold one open for ever.
    pub transaction_idle: Duration,
    /// Where `POST /v1/backups` puts backups, each in a directory of its
    /// own. Without it the server takes none: a client never chooses where
    /// on the server's disk files are written.
    pub backup_directory: Option<std::path::PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            token: None,
            transaction_idle: Duration::from_secs(60),
            backup_directory: None,
        }
    }
}

#[derive(Clone)]
struct Server {
    /// The database being served. A reader's is replaced when a newer
    /// publication is fetched; a request keeps the one it began with.
    store: StoreSlot,
    options: Arc<Options>,
    /// Open transactions and when each was last used.
    transactions: Arc<Mutex<HashMap<u64, Instant>>>,
}

/// Where the server keeps the database it serves, so that it can be
/// changed for another while the server runs.
#[derive(Clone)]
pub struct StoreSlot(Arc<std::sync::RwLock<Arc<RustStore>>>);

impl StoreSlot {
    /// The database being served now.
    pub fn get(&self) -> Arc<RustStore> {
        self.0.read().expect("store slot").clone()
    }

    /// Serve `store` from now on. Requests under way finish against the
    /// one they began with. A transaction open in the old one is lost.
    pub fn replace(&self, store: Arc<RustStore>) {
        *self.0.write().expect("store slot") = store;
    }
}

/// The service. Call [`expire_idle_transactions`] on a timer beside it.
pub fn router(store: Arc<RustStore>, options: Options) -> (Router, Janitor) {
    let server = Server {
        store: StoreSlot(Arc::new(std::sync::RwLock::new(store))),
        options: Arc::new(options),
        transactions: Arc::new(Mutex::new(HashMap::new())),
    };
    let janitor = Janitor {
        server: server.clone(),
    };
    let protected = Router::new()
        .route("/v1/query", post(query))
        .route("/v1/insert", post(insert))
        .route("/v1/script", post(script))
        .route("/v1/transactions", post(begin))
        .route("/v1/transactions/:id/commit", post(commit))
        .route("/v1/transactions/:id/rollback", post(rollback))
        .route("/v1/backups", post(backup))
        .layer(middleware::from_fn_with_state(server.clone(), authorize));
    let router = Router::new()
        .route("/v1/health", get(health))
        .merge(protected)
        .with_state(server);
    (router, janitor)
}

/// Rolls back transactions their clients have left idle.
pub struct Janitor {
    server: Server,
}

impl Janitor {
    /// Roll back every transaction idle for longer than the configured
    /// time, and return how many there were.
    pub fn expire_idle_transactions(&self) -> usize {
        let limit = self.server.options.transaction_idle;
        let idle: Vec<u64> = {
            let mut open = self.server.transactions.lock().expect("transaction table");
            let idle: Vec<u64> = open
                .iter()
                .filter(|(_, used)| used.elapsed() >= limit)
                .map(|(id, _)| *id)
                .collect();
            for id in &idle {
                open.remove(id);
            }
            idle
        };
        for id in &idle {
            let _ = self.server.store.get().rollback_transaction(*id);
        }
        idle.len()
    }

    /// The slot the served database is kept in.
    pub fn store(&self) -> StoreSlot {
        self.server.store.clone()
    }

    /// Check for idle transactions until the process ends.
    pub async fn run(self) {
        let every = (self.server.options.transaction_idle / 4).max(Duration::from_millis(50));
        let mut tick = tokio::time::interval(every);
        loop {
            tick.tick().await;
            self.expire_idle_transactions();
        }
    }
}

/// A failure, as the client sees it.
struct Failure {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl Failure {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request", message)
    }
}

impl From<OQLError> for Failure {
    fn from(error: OQLError) -> Self {
        let (status, code) = match &error {
            OQLError::TransactionConflict { .. } => (StatusCode::CONFLICT, "conflict"),
            OQLError::TransactionError { .. } => (StatusCode::NOT_FOUND, "no_such_transaction"),
            OQLError::DocumentNotFound { .. } => (StatusCode::NOT_FOUND, "not_found"),
            OQLError::SchemaValidation { .. }
            | OQLError::TypeError { .. }
            | OQLError::ReferenceError { .. }
            | OQLError::ParseError { .. }
            | OQLError::FunctionError { .. }
            | OQLError::QueryExecution { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "rejected"),
            // A reader's copy takes no writes. That is the caller's to
            // know, not a fault of the server.
            _ if error.to_string().contains("read-only copy") => (StatusCode::FORBIDDEN, "read_only"),
            _ => (StatusCode::INTERNAL_SERVER_ERROR, "internal"),
        };
        Self::new(status, code, error.to_string())
    }
}

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        (
            self.status,
            axum::Json(json!({ "error": { "code": self.code, "message": self.message } })),
        )
            .into_response()
    }
}

type Answer = Result<Json, Failure>;

async fn authorize(State(server): State<Server>, request: Request, next: Next) -> Response {
    if let Some(expected) = &server.options.token {
        let given = request
            .headers()
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        // Compare without stopping at the first difference.
        let matches = given.is_some_and(|given| {
            given.len() == expected.len()
                && given
                    .bytes()
                    .zip(expected.bytes())
                    .fold(0u8, |difference, (a, b)| difference | (a ^ b))
                    == 0
        });
        if !matches {
            return Failure::new(
                StatusCode::UNAUTHORIZED,
                "unauthorized",
                "a valid bearer token is required",
            )
            .into_response();
        }
    }
    next.run(request).await
}

async fn health() -> axum::Json<Json> {
    axum::Json(json!({ "status": "ok", "version": env!("CARGO_PKG_VERSION") }))
}

/// Run database work off the async threads and shape its outcome.
async fn blocking(work: impl FnOnce() -> Answer + Send + 'static) -> Response {
    match tokio::task::spawn_blocking(work).await {
        Ok(Ok(body)) => axum::Json(body).into_response(),
        Ok(Err(failure)) => failure.into_response(),
        Err(error) => Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            format!("the request did not finish: {error}"),
        )
        .into_response(),
    }
}

impl Server {
    /// Run `work` inside the client's transaction when it named one.
    fn within<T>(
        &self,
        transaction: Option<u64>,
        work: impl FnOnce() -> Result<T, Failure>,
    ) -> Result<T, Failure> {
        let Some(id) = transaction else {
            return work();
        };
        // Only transactions this server opened and still holds.
        match self.transactions.lock().expect("transaction table").get_mut(&id) {
            Some(used) => *used = Instant::now(),
            None => {
                return Err(Failure::new(
                    StatusCode::NOT_FOUND,
                    "no_such_transaction",
                    format!("transaction {id} is not open"),
                ))
            }
        }
        let store = self.store.get();
        let _bound = store.bind_transaction(id)?;
        work()
    }

    fn executor(&self) -> QueryExecutor<RustStore> {
        QueryExecutor::new_with_real_clock(self.store.get())
    }

    fn forget(&self, id: u64) -> bool {
        self.transactions
            .lock()
            .expect("transaction table")
            .remove(&id)
            .is_some()
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QueryRequest {
    query: String,
    #[serde(default)]
    params: Map<String, Json>,
    #[serde(default)]
    transaction: Option<u64>,
}

async fn query(State(server): State<Server>, axum::Json(request): axum::Json<QueryRequest>) -> Response {
    blocking(move || {
        let parsed = parse_oql(&request.query)
            .map_err(|error| Failure::new(StatusCode::BAD_REQUEST, "parse_error", error.to_string()))?;
        let params = request
            .params
            .iter()
            .map(|(name, value)| Ok((name.clone(), from_json(value)?)))
            .collect::<Result<HashMap<_, _>, Failure>>()?;
        let bound = bind_parameters(&parsed, &params).map_err(Failure::invalid)?;
        server.within(request.transaction, || {
            let executor = server.executor();
            let rows = executor.execute_query(&bound)?;
            Ok(json!({ "rows": rows_to_json(&executor, &rows)? }))
        })
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InsertRequest {
    collection: String,
    documents: Vec<Map<String, Json>>,
    #[serde(default)]
    transaction: Option<u64>,
}

async fn insert(State(server): State<Server>, axum::Json(request): axum::Json<InsertRequest>) -> Response {
    blocking(move || {
        let documents = request
            .documents
            .iter()
            .map(|document| {
                document
                    .iter()
                    .map(|(name, value)| Ok((name.clone(), from_json(value)?)))
                    .collect::<Result<owl_db::Fields, Failure>>()
            })
            .collect::<Result<Vec<_>, Failure>>()?;
        server.within(request.transaction, || {
            let inserted = server
                .executor()
                .insert_documents(&request.collection, documents)?;
            let ids: Vec<&str> = inserted.iter().map(|document| document.id.as_str()).collect();
            Ok(json!({ "inserted": inserted.len(), "ids": ids }))
        })
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ScriptRequest {
    source: String,
}

async fn script(State(server): State<Server>, axum::Json(request): axum::Json<ScriptRequest>) -> Response {
    blocking(move || {
        let file = parse_oql_file(&request.source)
            .map_err(|error| Failure::new(StatusCode::BAD_REQUEST, "parse_error", error.to_string()))?;
        let report = execute_oql_file(&file, &server.executor()).map_err(|error| {
            Failure::new(StatusCode::UNPROCESSABLE_ENTITY, "rejected", error.to_string())
        })?;
        if let Some(first) = report.errors.first() {
            return Err(Failure::new(
                StatusCode::UNPROCESSABLE_ENTITY,
                "rejected",
                first.clone(),
            ));
        }
        Ok(json!({
            "statements": report.results.len(),
            "schemas": report.schema_count,
            "queries": report.query_count,
            "functions": report.function_count,
        }))
    })
    .await
}

async fn begin(State(server): State<Server>) -> Response {
    blocking(move || {
        let id = server.store.get().begin_transaction()?;
        server
            .transactions
            .lock()
            .expect("transaction table")
            .insert(id, Instant::now());
        Ok(json!({ "transaction": id }))
    })
    .await
}

async fn commit(State(server): State<Server>, Path(id): Path<u64>) -> Response {
    blocking(move || {
        if !server.forget(id) {
            return Err(OQLError::TransactionError {
                message: format!("transaction {id} is not open"),
            }
            .into());
        }
        server.store.get().commit_transaction(id)?;
        Ok(json!({ "committed": true }))
    })
    .await
}

async fn rollback(State(server): State<Server>, Path(id): Path<u64>) -> Response {
    blocking(move || {
        if !server.forget(id) {
            return Err(OQLError::TransactionError {
                message: format!("transaction {id} is not open"),
            }
            .into());
        }
        server.store.get().rollback_transaction(id)?;
        Ok(json!({ "rolled_back": true }))
    })
    .await
}

async fn backup(State(server): State<Server>) -> Response {
    blocking(move || {
        let Some(directory) = &server.options.backup_directory else {
            return Err(Failure::new(
                StatusCode::CONFLICT,
                "backups_disabled",
                "this server was started without a backup directory",
            ));
        };
        // A name of the server's choosing: when it was taken, and a
        // sequence that keeps two taken in the same second apart.
        let taken = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let mut sequence = 0;
        let (name, destination) = loop {
            let name = format!("backup-{taken}-{sequence}");
            let destination = directory.join(&name);
            if !destination.exists() {
                break (name, destination);
            }
            sequence += 1;
        };
        let info = server.store.get().backup(&destination)?;
        Ok(json!({
            "name": name,
            "committed_lsn": info.committed_lsn,
            "files": info.files.len(),
            "bytes": info.bytes(),
        }))
    })
    .await
}

/// A JSON value as an Owl value. A whole number stays an integer; text
/// stays text, and becomes a timestamp or a reference only where the
/// schema says the field is one, exactly as in OQL.
fn from_json(value: &Json) -> Result<Value, Failure> {
    Ok(match value {
        Json::Null => Value::Null,
        Json::Bool(value) => Value::Boolean(*value),
        Json::String(value) => Value::String(value.clone()),
        Json::Number(number) => match (number.as_i64(), number.as_f64()) {
            (Some(integer), _) => Value::Integer(integer),
            (None, Some(float)) => Value::Float(float),
            (None, None) => {
                return Err(Failure::invalid(format!("{number} is not a number Owl can hold")))
            }
        },
        Json::Array(items) => Value::List(items.iter().map(from_json).collect::<Result<_, _>>()?),
        Json::Object(fields) => Value::Object(
            fields
                .iter()
                .map(|(name, value)| Ok((name.clone(), from_json(value)?)))
                .collect::<Result<_, Failure>>()?,
        ),
    })
}

/// An Owl value as JSON. A timestamp is RFC 3339 text and a reference is
/// the id of the document it points at.
fn to_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::Boolean(value) => json!(value),
        Value::Integer(value) => json!(value),
        // JSON has no NaN or infinity.
        Value::Float(value) => serde_json::Number::from_f64(*value).map_or(Json::Null, Json::Number),
        Value::String(value) => json!(value),
        Value::DateTime(instant) => json!(instant.to_rfc3339()),
        Value::List(items) => Json::Array(items.iter().map(to_json).collect()),
        Value::Object(fields) => Json::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), to_json(value)))
                .collect(),
        ),
        // Replaced by ids before this is reached.
        Value::Reference(_) | Value::GlobalReference(_) => Json::Null,
    }
}

fn rows_to_json(
    executor: &QueryExecutor<RustStore>,
    rows: &[owl_db::Document],
) -> Result<Vec<Json>, Failure> {
    rows.iter()
        .map(|row| {
            let mut object = Map::with_capacity(row.fields.len());
            for (name, value) in &row.fields {
                // Names starting with `_` are the executor's own.
                if name.starts_with('_') {
                    continue;
                }
                object.insert(name.clone(), to_json(&executor.with_reference_ids(value)?));
            }
            Ok(Json::Object(object))
        })
        .collect()
}
