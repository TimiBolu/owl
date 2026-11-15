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
| `--decoded-documents` | `OWL_DECODED_DOCUMENTS` | none | Hold at most this many documents decoded in memory; the rest are kept as records and decoded when read |
| `--backup-dir` | `OWL_BACKUP_DIR` | none | Where `POST /v1/backups` writes backups |
| `--restore-from` | | none | Restore this backup as `--data` before serving; `--data` must not exist |

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
