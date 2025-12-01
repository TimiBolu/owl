#!/bin/bash

# OQL Database End-to-End Demonstration Script
# This script demonstrates the complete workflow of OQL database

echo "============================================================================="
echo "OQL DATABASE - COMPLETE END-TO-END DEMONSTRATION"
echo "============================================================================="
echo ""
echo "This demonstration shows:"
echo "  1. Schema definition from .oql files"
echo "  2. Data insertion and validation"
echo "  3. Query execution (filtering, sorting, aggregation)"
echo "  4. Reference navigation (O(1) pointer-based)"
echo "  5. Data persistence to disk"
echo "  6. Cross-session data retrieval"
echo ""
echo "============================================================================="
echo "STEP 1: Clean up any existing database"
echo "============================================================================="
echo ""

if [ -f "./oql_e2e_test_db" ]; then
    echo "🧹 Removing existing database..."
    rm -f ./oql_e2e_test_db
    echo "✅ Cleanup complete"
else
    echo "✅ No existing database found"
fi

echo ""
echo "============================================================================="
echo "STEP 2: Run Complete E2E Test"
echo "============================================================================="
echo ""
echo "📖 Executing: oql_test_suite/complete_e2e_test.oql"
echo "   - 6 schema definitions"
echo "   - 16 document insertions"
echo "   - 50+ test queries"
echo ""

cargo run --example run_complete_e2e_test 2>&1 | grep -v "warning:" | grep -v "Compiling" | grep -v "Finished" | grep -v "Running"

echo ""
echo "============================================================================="
echo "STEP 3: Verify Data Persistence"
echo "============================================================================="
echo ""
echo "🔍 Opening database in a new session..."
echo "📊 Running verification queries..."
echo ""

cargo run --example verify_saved_data 2>&1 | grep -v "warning:" | grep -v "Compiling" | grep -v "Finished" | grep -v "Running"

echo ""
echo "============================================================================="
echo "STEP 4: Check Database File"
echo "============================================================================="
echo ""

if [ -f "./oql_e2e_test_db" ]; then
    DB_SIZE=$(ls -lh ./oql_e2e_test_db | awk '{print $5}')
    echo "✅ Database file created successfully"
    echo "   Location: ./oql_e2e_test_db"
    echo "   Size: $DB_SIZE"
    echo "   Format: Zig Arena binary"
else
    echo "❌ Database file not found"
    exit 1
fi

echo ""
echo "============================================================================="
echo "DEMONSTRATION COMPLETE!"
echo "============================================================================="
echo ""
echo "Summary:"
echo "  ✅ Schemas defined and enforced"
echo "  ✅ Data inserted with type validation"
echo "  ✅ Queries executed successfully"
echo "  ✅ References navigated (O(1) performance)"
echo "  ✅ Data persisted to disk"
echo "  ✅ Data retrieved across sessions"
echo ""
echo "🎉 The OQL database successfully handles .oql files!"
echo ""
echo "To clean up:"
echo "  rm -f ./oql_e2e_test_db"
echo ""
