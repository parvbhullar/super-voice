"""FastAPI application shell for the supervoice service.

Exposes:

* ``GET /health`` — liveness probe.
* ``WS  /call`` — stub for the WebRTC call handler. Real wiring lands in
  the follow-up task; for now the endpoint accepts the socket, returns a
  clear ``not_implemented`` event, and closes cleanly.

``Settings`` is intentionally constructed inside the lifespan context
manager (not at module import) so tests and tooling can import ``app``
without the runtime env vars being present.
"""

from __future__ import annotations

from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI, WebSocket
from loguru import logger

from supervoice.config import Settings


@asynccontextmanager
async def lifespan(app: FastAPI) -> AsyncIterator[None]:
    """Construct ``Settings`` once the app starts (not at import time)."""
    app.state.settings = Settings()  # pyright: ignore[reportCallIssue]
    logger.info("supervoice booted")
    yield


app = FastAPI(lifespan=lifespan)


@app.get("/health")
async def health() -> dict[str, str]:
    """Liveness probe used by container/orchestrator health checks."""
    return {"status": "ok"}


@app.websocket("/call")
async def call_endpoint(ws: WebSocket) -> None:
    """WebRTC signaling endpoint — real call handler lands in Task 11."""
    await ws.accept()
    await ws.send_json({"event": "not_implemented", "phase": 1})
    await ws.close()
