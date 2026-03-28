#!/usr/bin/env bash
# Validate that the active-call binary starts (--help) in under 1 second.
#
# Usage:
#   ./scripts/check_startup.sh [path-to-binary]
#
# Exit codes:
#   0 — startup time is under 1000 ms
#   1 — binary not found, or startup took >= 1000 ms
set -euo pipefail

BINARY="${1:-target/release/active-call}"

if [ ! -f "$BINARY" ]; then
    echo "ERROR: Binary not found at $BINARY"
    echo "  Build with: cargo build --release --features carrier"
    exit 1
fi

if ! command -v date >/dev/null 2>&1; then
    echo "ERROR: 'date' command not found"
    exit 1
fi

# Use nanosecond precision when available (Linux/macOS with GNU date or macOS date).
# On macOS, 'date +%s%N' is not supported by the BSD date built-in, so we fall
# back to perl for sub-second timing.
_now_ms() {
    if date +%s%N 2>/dev/null | grep -qE '^[0-9]+$'; then
        echo $(( $(date +%s%N) / 1000000 ))
    else
        perl -MTime::HiRes=time -e 'printf "%d\n", time()*1000'
    fi
}

START=$(_now_ms)

# Run the binary with --help; allow it to fail (exit non-zero) since we only
# care about startup time, not help text correctness.
timeout 5 "$BINARY" --help >/dev/null 2>&1 || true

END=$(_now_ms)

ELAPSED_MS=$(( END - START ))

echo "Startup time: ${ELAPSED_MS}ms"

if [ "$ELAPSED_MS" -gt 1000 ]; then
    echo "FAIL: Startup took ${ELAPSED_MS}ms (limit: 1000ms)"
    exit 1
fi

echo "PASS: Startup under 1 second"
