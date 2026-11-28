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
//! | `GET /v1/catalogue` | Says what is in the database: collections, fields, relationships, indexes, functions. |
//! | `POST /v1/check` | Says whether a statement would be accepted, without running it. |
//! | `POST /mcp` | The same four things as tools for a language model: see [`mcp`]. |
//!
//! `query` and `insert` take an optional `"transaction": id` and are then
//! part of that transaction. A statement outside a transaction is its own
//! commit and is atomic by itself. A commit answered with 409 and the code
//! `conflict` applied nothing: another commit changed a document the
//! transaction wrote, and the client runs the transaction again.
//!
//! Every failure is `{"error": {"code": .., "message": ..}}`. One of a
//! statement also carries `"problem"`: the same failure as data a program
//! can act on, with where it is and which names do exist.
//!
//! `query` takes `"read_only": true`, which refuses a statement that would
//! write, and `"max_ms"` and `"max_rows"`, which stop one that costs more.

use axum::extract::{Path, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use owl_db::error::OQLError;
use owl_db::storage::RustStore;
use owl_db::assist::{self, Problem};
use owl_db::{bind_parameters, execute_oql_file, parse_oql, parse_oql_file, Budget, QueryExecutor, Value};
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
    /// The most any statement sent to `/v1/query` may cost. A request may
    /// ask for less, never for more. None by default.
    pub budget: Budget,
    /// The most a statement a language model sends through `/mcp` may
    /// cost. Five seconds and a hundred thousand rows by default: a model
    /// writes a query to see what it gives, and one that pairs every row
    /// with every row should come back as an error, not as a wait.
    pub assistant_budget: Budget,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            token: None,
            transaction_idle: Duration::from_secs(60),
            backup_directory: None,
            budget: Budget::default(),
            assistant_budget: Budget {
                time: Some(Duration::from_secs(5)),
                rows: Some(100_000),
            },
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
        .route("/v1/catalogue", get(catalogue))
        .route("/v1/check", post(check))
        .route("/mcp", post(mcp::answer).get(mcp::no_stream))
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
    /// The same failure as data, when it is a statement's.
    problem: Option<Problem>,
}

impl Failure {
    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            problem: None,
        }
    }

    /// The failure of a statement that is not OQL.
    fn parse(text: &str, message: String) -> Self {
        let mut failure = Self::new(StatusCode::BAD_REQUEST, "parse_error", message.clone());
        failure.problem = Some(Problem::of_parse(text, message));
        failure
    }

    /// The failure of a statement the engine refused or stopped, with what
    /// the database knows about the names in it.
    fn of_statement(error: OQLError, executor: &QueryExecutor<RustStore>) -> Self {
        let known = assist::catalogue(executor).unwrap_or_default();
        let problem = Problem::of_error(&error, &known);
        let mut failure = Self::from(error);
        failure.problem = Some(problem);
        failure
    }

    /// A statement that would write, sent where only reads are taken.
    fn would_write() -> Self {
        let message = "this request takes only reads, and the statement would write";
        let mut failure = Self::new(StatusCode::FORBIDDEN, "read_only", message);
        failure.problem = Some(Problem::new("read_only", message));
        failure
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
            OQLError::BudgetExceeded { .. } => (StatusCode::UNPROCESSABLE_ENTITY, "over_budget"),
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
        let mut error = json!({ "code": self.code, "message": self.message });
        if let Some(problem) = &self.problem {
            error["problem"] = serde_json::to_value(problem).unwrap_or(Json::Null);
        }
        (self.status, axum::Json(json!({ "error": error }))).into_response()
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

    /// Run a statement that must not change the database, within `budget`.
    ///
    /// One that would write is refused before it runs. It is then run in
    /// a transaction that is rolled back, so that a write no check
    /// foresaw, in a function it calls, is undone all the same.
    fn read(
        &self,
        executor: &QueryExecutor<RustStore>,
        query: &owl_db::OQLQuery,
        budget: Budget,
    ) -> Result<Vec<owl_db::Document>, Failure> {
        if QueryExecutor::<RustStore>::writes(query) {
            return Err(Failure::would_write());
        }
        let store = self.store.get();
        let id = store.begin_transaction()?;
        let outcome = store
            .bind_transaction(id)
            .and_then(|_bound| owl_db::with_budget(budget, || executor.execute_query(query)));
        let _ = store.rollback_transaction(id);
        outcome.map_err(|error| Failure::of_statement(error, executor))
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
    /// Refuse the statement if it would write.
    #[serde(default)]
    read_only: bool,
    /// Stop the statement once it has run this many milliseconds.
    #[serde(default)]
    max_ms: Option<u64>,
    /// Stop the statement once it holds more rows than this at one time.
    #[serde(default)]
    max_rows: Option<usize>,
}

/// The lesser of two limits, where either may be absent.
fn lesser<T: Ord>(ours: Option<T>, theirs: Option<T>) -> Option<T> {
    match (ours, theirs) {
        (Some(ours), Some(theirs)) => Some(ours.min(theirs)),
        (ours, theirs) => ours.or(theirs),
    }
}

/// A statement's text with its parameters given values.
fn bound_statement(text: &str, params: &Map<String, Json>) -> Result<owl_db::OQLQuery, Failure> {
    let parsed = parse_oql(text).map_err(|error| Failure::parse(text, error.to_string()))?;
    let params = params
        .iter()
        .map(|(name, value)| Ok((name.clone(), from_json(value)?)))
        .collect::<Result<HashMap<_, _>, Failure>>()?;
    bind_parameters(&parsed, &params).map_err(Failure::invalid)
}

async fn query(State(server): State<Server>, axum::Json(request): axum::Json<QueryRequest>) -> Response {
    blocking(move || {
        let bound = bound_statement(&request.query, &request.params)?;
        let budget = Budget {
            time: lesser(server.options.budget.time, request.max_ms.map(Duration::from_millis)),
            rows: lesser(server.options.budget.rows, request.max_rows),
        };
        let executor = server.executor();
        // On its own, a read-only statement is rolled back whatever it
        // did. In the client's transaction it is refused if it would
        // write, and the transaction stays the client's to end.
        if request.read_only && request.transaction.is_none() {
            let rows = server.read(&executor, &bound, budget)?;
            return Ok(json!({ "rows": rows_to_json(&executor, &rows)? }));
        }
        if request.read_only && QueryExecutor::<RustStore>::writes(&bound) {
            return Err(Failure::would_write());
        }
        server.within(request.transaction, || {
            let rows = owl_db::with_budget(budget, || executor.execute_query(&bound))
                .map_err(|error| Failure::of_statement(error, &executor))?;
            Ok(json!({ "rows": rows_to_json(&executor, &rows)? }))
        })
    })
    .await
}

async fn catalogue(State(server): State<Server>) -> Response {
    blocking(move || {
        let catalogue = assist::catalogue(&server.executor())?;
        let mut body = serde_json::to_value(&catalogue).map_err(|error| Failure::invalid(error.to_string()))?;
        // The same, as a schema short enough to sit in a prompt.
        body["text"] = json!(catalogue.as_text());
        Ok(body)
    })
    .await
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckRequest {
    query: String,
}

/// `{"ok": true, ..}` with what the statement would do, or `{"ok": false,
/// "problem": ..}`. Both are answers, so both are 200.
async fn check(State(server): State<Server>, axum::Json(request): axum::Json<CheckRequest>) -> Response {
    blocking(move || {
        Ok(match assist::check(&server.executor(), &request.query) {
            Ok(checked) => json!({
                "ok": true,
                "collection": checked.collection,
                "writes": checked.writes,
                "parameters": checked.parameters,
            }),
            Err(problem) => json!({ "ok": false, "problem": problem }),
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
            // What the last statement gave, when it was a query or a
            // transaction block that ended in one.
            "rows": rows_to_json(&server.executor(), &report.rows)?,
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

/// Owl as tools for a language model, over the Model Context Protocol.
///
/// `POST /mcp` takes one JSON-RPC message and answers with one JSON body:
/// the protocol's HTTP transport with no stream. The server keeps nothing
/// between messages. It carries the same bearer token as every other
/// request.
///
/// | Tool | Does |
/// |---|---|
/// | `language` | OQL on one page. Read once, before the first query. |
/// | `describe` | What is in the database, as a schema. |
/// | `check` | Whether a statement would be accepted, without running it. |
/// | `explain` | How a query would run. |
/// | `query` | Runs a query that reads. |
///
/// Nothing sent here can change the database: a statement that would
/// write is refused, and every statement is run in a transaction that is
/// rolled back. Every statement is held to [`Options::assistant_budget`].
pub mod mcp {
    use super::*;

    /// OQL on one page, as the test suite runs it.
    pub const CARD: &str = include_str!("../../../docs/OQL_CARD.md");

    /// The revisions of the protocol this speaks, newest first. They
    /// differ in nothing a server with only tools and no stream uses.
    const VERSIONS: [&str; 4] = ["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

    /// The most rows `query` returns when not asked for fewer.
    const ROWS_RETURNED: usize = 100;

    const INSTRUCTIONS: &str = "Owl is a database queried in OQL, which is not SQL. \
        Call `language` once to read OQL on one page, and `describe` to see the collections, \
        fields and relationships, before writing a query. `check` says whether a query would be \
        accepted without running it, and what to change when it would not. `query` runs a query \
        that reads; nothing sent here can change the database.";

    fn tools() -> Json {
        let statement = json!({
            "type": "object",
            "properties": { "query": { "type": "string", "description": "One OQL statement." } },
            "required": ["query"],
        });
        let read_only = json!({ "readOnlyHint": true });
        json!([
            {
                "name": "language",
                "title": "OQL on one page",
                "description": "The OQL query language on one page, with examples that are known to run and the mistakes the database refuses. Read it once before writing the first query.",
                "inputSchema": { "type": "object", "properties": {} },
                "annotations": read_only,
            },
            {
                "name": "describe",
                "title": "What is in the database",
                "description": "Every collection as a schema: fields and types, which field refers to which collection, the name each relationship is followed by in either direction, indexes, row counts, and the functions defined here. Names not listed here do not exist.",
                "inputSchema": { "type": "object", "properties": {} },
                "annotations": read_only,
            },
            {
                "name": "check",
                "title": "Check a statement",
                "description": "Says whether a statement would be accepted, without running it. A refusal gives a code, the message, the line and column, the name that was probably meant, and the names that do exist.",
                "inputSchema": statement,
                "annotations": read_only,
            },
            {
                "name": "explain",
                "title": "Explain a query",
                "description": "How a query would run: whether an index serves it and, for a block with captures or a walk, how many rows each step made.",
                "inputSchema": statement,
                "annotations": read_only,
            },
            {
                "name": "query",
                "title": "Run a query that reads",
                "description": "Runs one OQL query and returns its rows as JSON. A statement that would write is refused. A query that runs too long or makes too many rows is stopped with an error that says so.",
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "One OQL statement." },
                        "params": { "type": "object", "description": "Values for the query's $name parameters." },
                        "limit": { "type": "integer", "description": "The most rows to return. 100 when absent, 1000 at most." },
                    },
                    "required": ["query"],
                },
                "annotations": read_only,
            },
        ])
    }

    /// What a tool gave: text for the model, and whether it is a failure
    /// of the statement, which the model can correct.
    type Outcome = Result<String, String>;

    fn refused(problem: &Problem) -> String {
        serde_json::to_string_pretty(problem).unwrap_or_else(|_| problem.message.clone())
    }

    fn failed(failure: Failure) -> String {
        match &failure.problem {
            Some(problem) => refused(problem),
            None => failure.message,
        }
    }

    fn call(server: &Server, name: &str, arguments: &Json) -> Option<Outcome> {
        let text = arguments.get("query").and_then(Json::as_str);
        let needs_query = || "this tool needs a `query`: one OQL statement".to_string();
        let executor = server.executor();
        let budget = server.options.assistant_budget;
        Some(match name {
            "language" => Ok(CARD.to_string()),
            "describe" => assist::catalogue(&executor)
                .map(|catalogue| catalogue.as_text())
                .map_err(|error| error.to_string()),
            "check" => text.ok_or_else(needs_query).and_then(|text| match assist::check(&executor, text) {
                Ok(checked) => Ok(json!({
                    "ok": true,
                    "writes": checked.writes,
                    "parameters": checked.parameters,
                })
                .to_string()),
                Err(problem) => Err(refused(&problem)),
            }),
            "explain" => text.ok_or_else(needs_query).and_then(|text| {
                assist::check(&executor, text).map_err(|problem| refused(&problem))?;
                let mut query = parse_oql(text).map_err(|error| error.to_string())?;
                if !matches!(query.operations.last(), Some(owl_db::Operation::Explain)) {
                    query.operations.push(owl_db::Operation::Explain);
                }
                let rows = server.read(&executor, &query, budget).map_err(failed)?;
                let rows = rows_to_json(&executor, &rows).map_err(failed)?;
                Ok(match rows.first().and_then(|row| row.get("plan")).and_then(Json::as_str) {
                    Some(plan) => plan.to_string(),
                    None => Json::Array(rows).to_string(),
                })
            }),
            "query" => text.ok_or_else(needs_query).and_then(|text| {
                let none = Map::new();
                let params = arguments.get("params").and_then(Json::as_object).unwrap_or(&none);
                // Checked as written first: a name that is not there is
                // then reported with the names that are.
                assist::check(&executor, text).map_err(|problem| refused(&problem))?;
                let bound = bound_statement(text, params).map_err(failed)?;
                let rows = server.read(&executor, &bound, budget).map_err(failed)?;
                let most = arguments
                    .get("limit")
                    .and_then(Json::as_u64)
                    .map_or(ROWS_RETURNED, |limit| (limit as usize).clamp(1, 1000));
                let shown = rows_to_json(&executor, &rows[..rows.len().min(most)]).map_err(failed)?;
                let mut answer = json!({ "rows": shown, "row_count": rows.len() });
                if rows.len() > most {
                    answer["note"] = json!(format!(
                        "the first {most} of {} rows; add a limit, or aggregate, to see less",
                        rows.len()
                    ));
                }
                Ok(answer.to_string())
            }),
            _ => return None,
        })
    }

    fn result(id: &Json, result: Json) -> Response {
        axum::Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
    }

    fn error(status: StatusCode, id: &Json, code: i64, message: impl Into<String>) -> Response {
        let body = json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message.into() } });
        (status, axum::Json(body)).into_response()
    }

    /// A page in a browser may only reach this server from the machine it
    /// runs on: a request that names another origin is refused, so a page
    /// elsewhere cannot use the browser to reach a server on loopback.
    fn origin_allowed(headers: &header::HeaderMap) -> bool {
        let Some(origin) = headers.get(header::ORIGIN).and_then(|value| value.to_str().ok()) else {
            return true;
        };
        let host = origin.split("://").nth(1).unwrap_or(origin);
        let host = host.strip_prefix('[').map_or_else(
            || host.split(':').next().unwrap_or(host),
            |rest| rest.split(']').next().unwrap_or(rest),
        );
        matches!(host, "localhost" | "127.0.0.1" | "::1")
    }

    /// `GET /mcp`: there is no stream to listen on.
    pub(super) async fn no_stream() -> Response {
        StatusCode::METHOD_NOT_ALLOWED.into_response()
    }

    pub(super) async fn answer(
        State(server): State<Server>,
        headers: header::HeaderMap,
        axum::Json(message): axum::Json<Json>,
    ) -> Response {
        if !origin_allowed(&headers) {
            return error(StatusCode::FORBIDDEN, &Json::Null, -32600, "requests from this origin are not taken");
        }
        if let Some(version) = headers.get("mcp-protocol-version").and_then(|value| value.to_str().ok()) {
            if !VERSIONS.contains(&version) {
                return error(
                    StatusCode::BAD_REQUEST,
                    &Json::Null,
                    -32600,
                    format!("unsupported protocol version {version}; this server speaks {}", VERSIONS.join(", ")),
                );
            }
        }
        let Some(method) = message.get("method").and_then(Json::as_str).map(str::to_string) else {
            // A response to something this server never asks, or a batch,
            // which the protocol no longer has.
            return match message.is_object() {
                true => StatusCode::ACCEPTED.into_response(),
                false => error(StatusCode::BAD_REQUEST, &Json::Null, -32600, "one JSON-RPC message is expected"),
            };
        };
        // A notification has no id and gets no answer.
        let Some(id) = message.get("id").cloned() else {
            return StatusCode::ACCEPTED.into_response();
        };
        let params = message.get("params").cloned().unwrap_or(Json::Null);
        match method.as_str() {
            "initialize" => {
                let asked = params.get("protocolVersion").and_then(Json::as_str).unwrap_or_default();
                let version = VERSIONS.iter().find(|known| **known == asked).unwrap_or(&VERSIONS[0]);
                result(
                    &id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": { "tools": {} },
                        "serverInfo": { "name": "owl", "title": "Owl", "version": env!("CARGO_PKG_VERSION") },
                        "instructions": INSTRUCTIONS,
                    }),
                )
            }
            "ping" => result(&id, json!({})),
            "tools/list" => result(&id, json!({ "tools": tools() })),
            "tools/call" => {
                let name = params.get("name").and_then(Json::as_str).unwrap_or_default().to_string();
                let arguments = params.get("arguments").cloned().unwrap_or(Json::Null);
                let called = {
                    let name = name.clone();
                    tokio::task::spawn_blocking(move || call(&server, &name, &arguments)).await
                };
                match called {
                    Ok(Some(outcome)) => {
                        let failed = outcome.is_err();
                        let text = outcome.unwrap_or_else(|text| text);
                        result(&id, json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
                    }
                    Ok(None) => error(StatusCode::OK, &id, -32602, format!("Unknown tool: {name}")),
                    Err(crashed) => error(StatusCode::OK, &id, -32603, format!("the tool did not finish: {crashed}")),
                }
            }
            other => error(StatusCode::OK, &id, -32601, format!("Method not found: {other}")),
        }
    }
}
