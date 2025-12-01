# Disk Space Management

## Problem

The Owl project can accumulate large amounts of disk space due to:

1. **Rust build artifacts** (`target/` directories) - can reach 15-20GB
2. **Zig test backup files** (`.zig-cache/tmp/*.owlb`) - individual files can be 16-33GB each!
3. **Zig build cache** (`.zig-cache/`) - typically 500MB-1GB

## Quick Fix

Run the cleanup script:

```bash
./clean_all.sh
```

This will:
- Clean all Rust build artifacts (`cargo clean`)
- Remove large Zig backup test files (`*.owlb`)
- Remove large test arena files (`*.dat` > 100MB)
- Show you the final disk usage

## Preventing the Problem

### For Zig Development

If you're working in `owl_storage_zig/`, run:

```bash
cd owl_storage_zig
./clean_test_files.sh
```

This removes just the Zig test files without touching Rust artifacts.

### Why Do Backup Files Get So Large?

The backup tests in `src/backup_recovery.zig` create full backups of test arenas. While the test arenas themselves are only 1MB, the backup format writes the entire arena memory map, which can expand to 16-33GB per backup file.

These files are created in `.zig-cache/tmp/` and are **not automatically cleaned up** by Zig's test runner.

### .gitignore

The following patterns are already in `.gitignore` to prevent committing these files:

```
*.owlb
owl_storage_zig/.zig-cache/tmp/*.owlb
```

## Disk Usage Monitoring

Check current usage:

```bash
# Total project size
du -sh .

# Rust artifacts
du -sh target

# Zig cache
du -sh owl_storage_zig/.zig-cache

# Find large backup files
find owl_storage_zig/.zig-cache/tmp -name "*.owlb" -exec ls -lh {} \;
```

## Best Practices

1. **Run `./clean_all.sh` regularly** - especially after running tests
2. **Before committing** - check that no `.owlb` files are staged
3. **CI/CD** - ensure cleanup runs after test suites
4. **Development** - consider running `cargo clean` weekly if not actively building

## Future Improvements

Consider:
- Modifying backup tests to use smaller arena sizes
- Adding automatic cleanup in test teardown
- Implementing backup file rotation/cleanup in the backup module
- Adding a pre-commit hook to warn about large files
