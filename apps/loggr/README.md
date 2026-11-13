# Loggr

Loggr is Owl's first external application: a local-first error investigation
workbench for structured logs, exceptions, stack traces, issues, and trace
context. It uses only Owl's public Rust API.

The OQL workbench consumes Owl's batch-native query results and serializes
projected rows directly to the response buffer. Its timing strip separates
planning, execution, row materialization, and JSON serialization so broad
output cost is not confused with storage-engine work.

## Run it

```sh
cargo run -p loggr -- serve --db ./loggr.owl
```

In another terminal, seed the built-in checkout scenario and open the printed
URL:

```sh
cargo run -p loggr -- demo --count 240
```

Send one JSON event or an array from disk:

```sh
cargo run -p loggr -- send ./event.json
```

After stopping Loggr, check the authoritative Owl files:

```sh
cargo run -p loggr -- check --db ./loggr.owl
```

## Ingest API

- `POST /api/v1/events` accepts one JSON event (up to 1 MiB).
- `POST /api/v1/events/batch` accepts a JSON array or NDJSON (up to 256 events
  and 8 MiB).
- `202 Accepted` means the event and its issue update were durably committed in
  one Owl batch.
- Retried event IDs return `duplicate: true` without incrementing an issue.

The fuller product, data, and performance contract lives in
[`../../docs/LOGGR_PRODUCT_AND_ARCHITECTURE.md`](../../docs/LOGGR_PRODUCT_AND_ARCHITECTURE.md).
