#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
uv run uvicorn supervoice.main:app --host 0.0.0.0 --port 8080 --reload
