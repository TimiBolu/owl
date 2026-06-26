# Owl Explorer Vector Index API

These endpoints are available on `owl-explorer` server.

## List vector indexes

`POST /api/vector/indexes/list`

```json
{
  "database": "toy_app",
  "schema": "products"
}
```

## Upsert vector index

`POST /api/vector/indexes/upsert`

```json
{
  "database": "toy_app",
  "schema": "products",
  "name": "idx_products_embedding",
  "field": "embedding",
  "dimension": 3,
  "metric": "cosine",
  "index_type": "exact",
  "quantization": null,
  "state": "ready"
}
```

## Delete vector index

`POST /api/vector/indexes/delete`

```json
{
  "database": "toy_app",
  "schema": "products",
  "name": "idx_products_embedding"
}
```

## Notes

- Index metadata is WAL-backed and recovers after restart.
- Query execution currently uses exact nearest path with Zig kernel.
- Optional Mojo acceleration is runtime-loaded with `OWL_MOJO_KERNELS_LIB`.
