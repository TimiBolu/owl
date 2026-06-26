#!/bin/bash
# Clean up large test files that accumulate in .zig-cache

echo "Cleaning up large test files..."

# Remove backup files (can be 16-33GB each!)
find .zig-cache/tmp -name "*.owlb" -delete 2>/dev/null && echo "✓ Removed .owlb backup files"

# Remove large .dat files (> 100MB)
find .zig-cache/tmp -name "*.dat" -size +100M -delete 2>/dev/null && echo "✓ Removed large .dat files"

# Show current cache size
echo ""
echo "Current .zig-cache size:"
du -sh .zig-cache 2>/dev/null || echo "No .zig-cache directory"

echo ""
echo "Done! Run 'zig build test' to regenerate test files if needed."
