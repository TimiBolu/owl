# Owl Transaction Semantics (P0)

This document defines the current transaction behavior implemented for P0.

## OQL syntax

```oql
transaction {
  users.insert { id: "u1", name: "Ada" }
  users.where { .id == "u1" }.update { name: "Ada Lovelace" }
}
```

## Guarantees in this phase

- Atomicity (script transaction block):
  - All statements inside `transaction { ... }` are applied together or rolled back together.
  - Any statement failure rolls back the entire block.

- Durability (WAL markers):
  - Storage writes are tagged with a transaction id when a local transaction is active.
  - WAL includes `TxnBegin`, `TxnCommit`, `TxnRollback` markers.
  - Recovery replays:
    - autocommit entries, and
    - transaction-tagged entries only if a matching `TxnCommit` marker exists.
  - Open transactions without commit markers are ignored on restart.

- Crash consistency:
  - Torn/corrupt WAL tail is tolerated by replaying only complete valid frames.
  - Partial or uncommitted transaction state is not applied.

## Isolation level in P0

- Effective model: single active local transaction per `ZigStore` instance.
- Concurrent second `begin` while one is active is rejected.
- This gives serial execution for local explicit transactions.

## Current limits (intentional for P0)

- No nested transactions.
- No savepoints.
- No multi-transaction optimistic/locking scheduler yet.
- No cross-node distributed transaction semantics in this P0 surface.

## Validation coverage

- Recovery ignores uncommitted/open transaction writes.
- Recovery applies committed transaction writes.
- Concurrent begin race rejects second active transaction.
- Parser/executor rollback tests verify no partial writes for failed transaction blocks.
