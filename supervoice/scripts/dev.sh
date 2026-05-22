#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."

# Start supervoice in single-process dev mode
export SUPERVOICE_API_SECRETS="dev-mode:dev-secret"
export DEEPGRAM_API_KEY="${DEEPGRAM_API_KEY:-dummy}"
export CARTESIA_API_KEY="${CARTESIA_API_KEY:-dummy}"

echo "Starting supervoice in dev mode..."
echo "Health check: curl http://localhost:8080/health"
echo "Dispatch:     curl -X POST http://localhost:8080/v1/dispatch ..."
echo ""

uv run uvicorn supervoice.orchestrator.main:app --host 0.0.0.0 --port 8080
