#!/bin/bash
# Run comprehensive benchmark suite

set -e

echo "Building benchmarks..."
zig build -Doptimize=ReleaseFast

echo ""
echo "Running benchmark suite..."
echo ""

./zig-out/bin/bench

echo ""
echo "Benchmarks complete!"
echo ""
echo "To compare with Rust implementation:"
echo "  cd ../owl_storage_engine"
echo "  cargo bench --bench storage_benchmarks"
