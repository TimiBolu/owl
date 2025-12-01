#!/bin/bash
# Comprehensive cleanup script for the Owl project
# Removes build artifacts and large test files

echo "🧹 Cleaning Owl project..."
echo ""

# Clean Rust build artifacts
echo "Cleaning Rust build artifacts..."
cargo clean
echo "✓ Rust artifacts cleaned"
echo ""

# Clean Zig cache and large test files
echo "Cleaning Zig cache and test files..."
if [ -d "owl_storage_zig/.zig-cache" ]; then
    # Remove large backup files (can be 16-33GB each!)
    find owl_storage_zig/.zig-cache/tmp -name "*.owlb" -delete 2>/dev/null
    echo "✓ Removed .owlb backup files"
    
    # Remove large .dat files (> 100MB)
    find owl_storage_zig/.zig-cache/tmp -name "*.dat" -size +100M -delete 2>/dev/null
    echo "✓ Removed large .dat files"
fi
echo ""

# Show final sizes
echo "📊 Final sizes:"
echo "  Rust target:      $(du -sh target 2>/dev/null | cut -f1 || echo 'cleaned')"
echo "  Zig cache:        $(du -sh owl_storage_zig/.zig-cache 2>/dev/null | cut -f1 || echo 'none')"
echo "  Total project:    $(du -sh . 2>/dev/null | cut -f1)"
echo ""

echo "✨ Done! Your laptop thanks you."
