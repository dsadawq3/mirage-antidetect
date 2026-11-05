#!/usr/bin/env bash
# Mirage smoke test: rustfmt check, workspace build, workspace tests.
# Runs every step even if a previous one failed, prints a per-step verdict
# and a final summary.
#
# Exit codes:
#   0 - all steps passed
#   1 - at least one step failed
#   2 - environment problem (cargo not found / not a Mirage checkout)

set -euo pipefail

# The repository root is the parent of the directory holding this script.
REPO_ROOT="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"

# Preflight: cargo must be available and this must be a workspace checkout.

if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found in PATH." >&2
    echo "Install the Rust toolchain first, then re-run this script:" >&2
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
    exit 2
fi

if [ ! -f "$REPO_ROOT/Cargo.toml" ]; then
    echo "ERROR: Cargo.toml not found at $REPO_ROOT/Cargo.toml" >&2
    echo "This script must stay inside the scripts/ directory of the Mirage repository." >&2
    exit 2
fi

cd -- "$REPO_ROOT"

STEPS_TOTAL=3
STEPS_PASSED=0
STEPS_FAILED=0
FAILED_STEPS=""

# Step 1: rustfmt check
echo "Step 1/3: cargo fmt --all -- --check"
if cargo fmt --all -- --check; then
    echo "Step 1/3: PASSED"
    STEPS_PASSED=$((STEPS_PASSED + 1))
else
    CODE=$?
    echo "Step 1/3: FAILED: cargo fmt exited with code $CODE" >&2
    echo "  Formatting does not match rustfmt.toml, or the rustfmt component is missing." >&2
    echo "  Fix: cargo fmt --all" >&2
    echo "  Component (if missing): rustup component add rustfmt" >&2
    STEPS_FAILED=$((STEPS_FAILED + 1))
    FAILED_STEPS="fmt"
fi

# Step 2: workspace build
echo "Step 2/3: cargo build --workspace"
if cargo build --workspace; then
    echo "Step 2/3: PASSED"
    STEPS_PASSED=$((STEPS_PASSED + 1))
else
    CODE=$?
    echo "Step 2/3: FAILED: cargo build exited with code $CODE" >&2
    echo "  The workspace failed to compile; read the compiler errors above." >&2
    echo "  If workspace members are missing (crates/*/Cargo.toml), the root" >&2
    echo "  Cargo.toml cannot be loaded and every cargo command will fail." >&2
    STEPS_FAILED=$((STEPS_FAILED + 1))
    FAILED_STEPS="${FAILED_STEPS:+$FAILED_STEPS, }build"
fi

# Step 3: workspace tests
echo "Step 3/3: cargo test --workspace"
if cargo test --workspace; then
    echo "Step 3/3: PASSED"
    STEPS_PASSED=$((STEPS_PASSED + 1))
else
    CODE=$?
    echo "Step 3/3: FAILED: cargo test exited with code $CODE" >&2
    echo "  Tests failed or the workspace failed to compile; read the output above." >&2
    STEPS_FAILED=$((STEPS_FAILED + 1))
    FAILED_STEPS="${FAILED_STEPS:+$FAILED_STEPS, }test"
fi

echo ""
echo "Summary: $STEPS_PASSED of $STEPS_TOTAL steps passed."
if [ "$STEPS_FAILED" -gt 0 ]; then
    echo "Failed steps: $FAILED_STEPS"
    exit 1
fi

exit 0
