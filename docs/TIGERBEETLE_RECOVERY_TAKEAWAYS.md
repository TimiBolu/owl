# TigerBeetle Recovery Takeaways Applied to Owl

## Local reference import

TigerBeetle source is now vendored at:

- `vendor/tigerbeetle`

Primary files studied:

- `vendor/tigerbeetle/src/vsr/superblock.zig`
- `vendor/tigerbeetle/src/vsr/superblock_quorums.zig`
- `vendor/tigerbeetle/src/vsr/journal.zig`
- `vendor/tigerbeetle/src/vsr/replica.zig`
- `vendor/tigerbeetle/src/vsr/client_replies.zig`
- `vendor/tigerbeetle/src/vsr/grid_scrubber.zig`

## Changes already applied in Owl

1. Arena bootstrap recovery
- If arena open fails but WAL exists, Owl recreates arena and replays WAL.

2. Torn-tail tolerant WAL replay
- Recovery now ignores incomplete/corrupt tail entries instead of hard-failing startup.

3. Framed + checksummed WAL entries
- New WAL entries now use: magic + version + metadata + payload + CRC32 validation.
- Recovery supports both new framed WAL and legacy `[len][payload]` format.

4. Superblock copyset with startup policy
- Owl maintains a `.sb` sidecar with 4 copies and startup copy selection.
- `OWL_SUPERBLOCK_POLICY=availability` (default): fallback to singleton copy if quorum missing.
- `OWL_SUPERBLOCK_POLICY=strict`: requires quorum-valid copyset; fails open otherwise.

## Tests added/updated

- `test_storage_recovers_with_truncated_wal_tail`
- `test_storage_recovers_legacy_wal_format`

## Next TigerBeetle-inspired steps (recommended)

1. Add persistent metadata copyset (multiple header copies + sequence/checksum/parent chain).
2. Add startup quorum selection over metadata copies (single-node copy quorum, not cluster quorum).
3. Add background scrubber for periodic object/page verification and quarantine.
4. Add structured recovery report surfaced in Owl Explorer UI.
