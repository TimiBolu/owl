# owl-server

Owl as a network service. One process owns the database file and answers
JSON over HTTP, so programs in any language can use it.

```bash
cargo run --release -p owl-server -- --data shop.owl --listen 127.0.0.1:7878 --token "$OWL_TOKEN"
```

| Flag | Environment | Default | Meaning |
|---|---|---|---|
| `--data` | `OWL_DATA` | `owl.db` | The database file, created if absent |
| `--listen` | `OWL_LISTEN` | `127.0.0.1:7878` | Address to serve on |
| `--token` | `OWL_TOKEN` | none | Bearer token every request but the health check must carry |
| `--max-gib` | | 64 | The most the database may grow to |
| `--transaction-idle-seconds` | | 60 | Roll back a transaction left untouched this long |
| `--query-ms`, `--query-rows` | | none | Stop any statement that runs longer or holds more rows at once |
| `--assistant-ms`, `--assistant-rows` | | 5000, 100000 | The same for statements sent through `/mcp` |
| `--decoded-documents` | `OWL_DECODED_DOCUMENTS` | none | Hold at most this many documents decoded in memory. The rest are decoded when read, from records read back from the checkpoint files, and a writer waits for overdue checkpoints so that memory stays bounded while loading |
| `--backup-dir` | `OWL_BACKUP_DIR` | none | Where `POST /v1/backups` writes backups |
| `--restore-from` | | none | Restore this backup as `--data` before serving; `--data` must not exist |
| `--s3-bucket` and others | | none | Keep the database in an object store; see below |

The server refuses to listen on anything but loopback without a token. It
speaks plain HTTP: put it behind a TLS-terminating proxy to cross a network
you do not trust.

## Requests

| Request | Body | Answer |
|---|---|---|
| `GET /v1/health` | | `{"status": "ok", "version": ..}` |
| `POST /v1/query` | `{"query": "..", "params": {..}, "transaction": id}` | `{"rows": [..]}` |
| `POST /v1/insert` | `{"collection": "..", "documents": [..], "transaction": id}` | `{"inserted": n, "ids": [..]}` |
| `POST /v1/script` | `{"source": ".."}` | counts of what ran |
| `POST /v1/transactions` | `{}` | `{"transaction": id}` |
| `POST /v1/transactions/{id}/commit` | `{}` | `{"committed": true}` |
| `POST /v1/transactions/{id}/rollback` | `{}` | `{"rolled_back": true}` |
| `POST /v1/backups` | `{}` | `{"name": .., "committed_lsn": n, "files": n, "bytes": n}` |
| `GET /v1/catalogue` | | what is in the database; see below |
| `POST /v1/check` | `{"query": ".."}` | `{"ok": true, ..}` or `{"ok": false, "problem": {..}}` |
| `POST /mcp` | one JSON-RPC message | the tools a language model uses; see below |

`params` and `transaction` are optional.

- **Values go in `params`.** A query names them as `$name`. A parameter is
  never read as OQL, whatever text it holds.
- **JSON in.** A whole number is an integer, and becomes a float where the
  field is a `Float`. Text becomes a timestamp or a reference where the
  schema says the field is one, as in OQL: a reference is written as the id
  it points at.
- **JSON out.** A timestamp is RFC 3339 text and a reference is the id of
  its target.
- **`/v1/script`** runs schema declarations, functions and statements in
  order. It is how a schema is created or changed.

## For a program that writes queries

A person reads a schema file and an error message. A program that writes
OQL, a language model above all, needs the same things as data.

**The catalogue.** `GET /v1/catalogue` gives every collection with its
fields and their types, which field refers to which collection, the name
each relationship is followed by from the other end, its indexes and its
row count, and the functions defined in the database. `"text"` in the
answer is the same as a schema, short enough to put in a prompt:

```text
schema people {  // 300 rows
  id: String
  manager: Optional(Ref(people))
  name: String
  // .reports: the people whose .manager is this row
  // index, ordered: level
}
```

It reads schemas, index definitions and counts, and no rows.

**The check.** `POST /v1/check` says whether a statement would be
accepted, and runs nothing: nothing is read and nothing is written, so it
costs the same on any amount of data. A statement that passes comes back
with whether it would write and which `$name` parameters it needs. One
that does not comes back with a problem:

```json
{ "ok": false, "problem": {
  "code": "unknown_name",
  "message": "'people' has no field or relationship 'levle'. Did you mean 'level'?",
  "suggestion": "level",
  "names": { "people": ["id", "level", "manager", "name", "reports"] } } }
```

| Code | The statement |
|---|---|
| `parse_error` | is not OQL; `line` and `column` say where |
| `unknown_collection` | names a collection that is not there |
| `unknown_name` | reads a field or relationship that is not there |
| `unknown_function` | calls a function that is not defined |
| `type_mismatch` | compares kinds that could never be equal |
| `read_only` | would write, where only reads are taken |
| `over_budget` | was stopped at its budget |
| `rejected` | was refused for another reason; see the message |

These are the checks every statement gets before it runs, so the check
refuses nothing that would have run. A statement that passes can still
fail on a value: a division by zero, a constraint, a budget.

A statement that fails at `/v1/query` keeps the `code` it always had and
now carries the same `problem` beside it, under `error`.

**Reads that cannot write, and a budget.** `/v1/query` takes three more
fields:

| Field | Does |
|---|---|
| `"read_only": true` | Refuses a statement that would write, with `403` and `read_only`. Sent outside a transaction, the statement is also run in one that is rolled back, so nothing it calls can write either |
| `"max_ms": n` | Stops the statement once it has run that long |
| `"max_rows": n` | Stops the statement once it holds more rows than that at one time |

A statement stopped at its budget fails with `over_budget` and has written
nothing. `--query-ms` and `--query-rows` set a budget for every statement;
a request can ask for less and never for more. The budget bounds the work
that grows faster than the data: the rows a block's captures make and the
queries nested in a row. One read of a collection is not interrupted part
way.

### Tools for a language model

`POST /mcp` speaks the Model Context Protocol, so an assistant that takes
MCP servers can use the database directly. It is the protocol's HTTP
transport answered with plain JSON: one message in, one out, nothing kept
between them. It carries the same bearer token as every other request.

```bash
claude mcp add --transport http owl http://127.0.0.1:7878/mcp --header "Authorization: Bearer $OWL_TOKEN"
```

| Tool | Does |
|---|---|
| `language` | OQL on one page, `docs/OQL_CARD.md`: every example on it is run by the test suite |
| `describe` | The catalogue as text |
| `check` | The check |
| `explain` | How a query would run |
| `query` | Runs a query that reads; at most 100 rows come back unless `limit` asks for more, up to 1000 |

Nothing sent to `/mcp` can change the database: every statement is run
read-only, as above. Every statement is held to a budget of five seconds
and 100,000 rows, which `--assistant-ms` and `--assistant-rows` change. A
refusal comes back to the model as the problem above, so it can correct
the query itself.

A request to `/mcp` that names an `Origin` other than this machine is
refused, so a page in a browser cannot reach a server on loopback.

## Transactions

A statement sent without a transaction is its own commit and is atomic by
itself: `update { stock: .stock - 1 }` from two clients takes two off, and
`where { .version == 3 }.update { .. }` is a compare-and-set.

A transaction makes several statements one step. What it writes is seen
only by statements sent with its id until it commits. Its reads see what is
committed at the time of each read, plus its own writes.

A commit answered `409` with the code `conflict` applied nothing: another
commit changed a document this transaction wrote after it began. The
transaction is over, and the client runs it again from the start. Only
documents a transaction wrote are checked, not ones it only read.

## Backup and restore

Start the server with `--backup-dir` and `POST /v1/backups` copies the
database, as of its last commit, into a new directory there. Commits wait
while the files are copied; reads do not. The server chooses the directory's
name, so a client cannot write anywhere else on its disk. A transaction that
has not committed is not in the backup.

A backup is a directory of the database's files and a manifest of their
sizes and checksums. Copy it wherever backups are kept.

To come back from one, start a server with `--restore-from <backup>` and a
`--data` path that does not exist. The backup is checked against its
manifest, copied, opened and audited before the server answers anything. A
backup that is damaged or incomplete is refused, and nothing is ever
restored over an existing database.

## A database in an object store

A server can keep the database's large files, its checkpoint parts, in an
S3 bucket as well as on its own disk. Other servers can then serve the same
database for reading, with nothing but the bucket.

| Option | Environment | What it does |
|---|---|---|
| `--s3-bucket` | `OWL_S3_BUCKET` | Keep the parts in this bucket |
| `--s3-region` | `OWL_S3_REGION` | The bucket's region; `us-east-1` by default |
| `--s3-endpoint` | `OWL_S3_ENDPOINT` | Host and port of a server that speaks S3 and is not AWS |
| `--s3-insecure` | | Reach `--s3-endpoint` over plain HTTP |
| `--s3-prefix` | `OWL_S3_PREFIX` | What every key begins with in the bucket |
| `--objects-dir` | `OWL_OBJECTS_DIR` | Use a directory as the object store, one file an object |
| `--read-only` | | Serve what was last published; take no writes |
| `--take-over` | | Become the writer, starting from what was last published |
| `--object-cache-mib` | | Most the local cache of parts may hold; 256 by default |
| `--checkpoint-seconds` | `OWL_CHECKPOINT_SECONDS` | Checkpoint this often when something has been written |
| `--refresh-seconds` | `OWL_REFRESH_SECONDS` | With `--read-only`: look this often for a newer publication |

The access key and secret are read from `AWS_ACCESS_KEY_ID` and
`AWS_SECRET_ACCESS_KEY`.

**One server writes.** Started with a bucket and neither `--read-only` nor
`--take-over`, a server claims the database. It publishes after every
checkpoint, which is when readers can see what it has written. A second
server started the same way against the same bucket is refused.

Left alone, a server checkpoints by size: at about 16 MB of log or 10,000
commits. Give the writer `--checkpoint-seconds` to checkpoint on a timer
as well, whenever something has been written. That is what bounds how far
behind readers can be.

**Any number read.** A server started with `--read-only` fetches the last
publication and serves it. It refuses writes with `403` and the code
`read_only`. With `--refresh-seconds` it looks that often for a newer
publication and serves it when there is one, without restarting; without
it, it shows the database as published when the server started.

A reader keeps its copies of publications in `<data>.copies`, each in a
directory of its own, and removes one once two newer ones have been
fetched. A request under way when a newer one is swapped in finishes
against the copy it began with.
The parts it has read from the bucket are cached in one place for all of
them, so a newer publication reads again only what is new.

With both timers, a reader is behind the writer by at most about the two
intervals added together, plus the time to publish and fetch.

**Taking over.** If the writer is gone, start another with `--take-over`.
It begins from the last publication, so whatever the old writer committed
after its last checkpoint is not carried over. If the old writer is still
running it keeps answering from its own disk, but it can no longer publish
and cannot be started again as the writer.

```bash
owl-server --data /var/owl/shop.owl --s3-bucket my-bucket --s3-prefix shop/ --checkpoint-seconds 10
```

```bash
owl-server --data /var/owl-reader/shop.owl --s3-bucket my-bucket --s3-prefix shop/ --read-only --refresh-seconds 10 --listen 127.0.0.1:7879
```

Checked against MinIO. Not yet run against AWS itself.

## Failures

Every failure is `{"error": {"code": .., "message": ..}}`.

| Status | Code | Meaning |
|---|---|---|
| 400 | `parse_error` | The query or script is not valid OQL |
| 400 | `invalid_request` | A parameter is missing or a value cannot be held |
| 401 | `unauthorized` | The bearer token is missing or wrong |
| 404 | `no_such_transaction` | The transaction is not open: finished, expired or never begun |
| 409 | `conflict` | The commit lost to another; run the transaction again |
| 409 | `backups_disabled` | The server was started without `--backup-dir` |
| 422 | `rejected` | Owl refused the statement: a type, a constraint, an unknown field |
| 500 | `internal` | The storage engine failed |
