# OQL P0/P1/P2 Plan

## P0 (Correctness, no semantic ambiguity)
- Enforce strict schema type parsing for file schema definitions.
  - Done: unknown types now fail instead of silently becoming `String`.
  - Acceptance: invalid type in `schema ...` returns parse/type error.
- Prevent false success for unimplemented function-definition execution in file mode.
  - Done: function definitions in `execute_oql_file` are now registered on a mutable script executor.
  - Acceptance: `fn` statements succeed only on actual registration; subsequent statements can use them.
- Remove silent-true behavior for unsupported `MatchPattern::Guard`.
  - Done: returns explicit error.
  - Acceptance: guard pattern path no longer auto-matches.

## P1 (Feature velocity, language consistency)
- Single source of truth for field type grammar/coercion across parser + explorer API handlers.
  - Done: shared `src/type_syntax.rs` parser consumed by parser and explorer API handlers.
  - Acceptance: one parser for `List(...)`, `Optional(...)`, `Ref(...)`, aliases.
- Function execution in script mode.
  - Done: mutable function registry lifecycle in file/script execution path.
  - Acceptance: `fn` in script can be used by subsequent queries in same script.
- Language/docs sync pass.
  - Done: aligned canonical language docs and added executable phase script coverage.
  - Acceptance: `oql/phase1_acceptance.oql` parses and executes as CI-style smoke coverage.

## P2 (Performance and architecture)
- Reference coercion/indexing optimization.
  - Target: avoid full collection scans when coercing `String -> Ref`.
  - Acceptance: insert/update ref coercion no longer scales O(n) by collection size.
- Zig-backed index backend wiring.
  - Target: replace in-memory fallback path in production.
  - Acceptance: index operations route through Zig backend trait implementation.
- Parser/planner extensibility for new operations.
  - Target: operation registry pattern for easier syntax additions.
  - Acceptance: adding an operation requires localized changes + conformance tests.
