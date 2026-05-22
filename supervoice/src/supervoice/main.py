"""Thin entry point for the supervoice service.

Delegates to the V2 orchestrator ``create_app()`` factory, providing
default/stub service instances so the app can be imported without
requiring external infrastructure or specific env vars at import time.

V1 consumers (tests, ``uvicorn supervoice.main:app``) continue to work
unchanged.
"""

from __future__ import annotations

import os
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.main import create_app
from supervoice.orchestrator.mapping.cache import (
    NumberMappingCache,
)
from supervoice.orchestrator.room.in_process_engine import (
    InProcessRoomEngine,
)
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.worker_registry.dispatch import (
    WorkerDispatcher,
)
from supervoice.orchestrator.worker_registry.registry import WorkerRegistry


def _build_app() -> FastAPI:
    """Construct the orchestrator app with env-var-based config.

    Uses safe defaults for all services so tests that don't set
    orchestrator-specific env vars still get a bootable app.
    """
    auth_config = AuthConfig.from_env(os.environ.get("SUPERVOICE_API_SECRETS"))
    room_engine = InProcessRoomEngine()
    mapping_cache = NumberMappingCache()
    worker_registry = WorkerRegistry()
    worker_dispatcher = WorkerDispatcher(worker_registry)
    session_registry = SessionRegistry()

    inner = create_app(
        auth_config=auth_config,
        room_engine=room_engine,
        mapping_cache=mapping_cache,
        worker_dispatcher=worker_dispatcher,
        session_registry=session_registry,
    )

    # Preserve V1 lifespan behavior: construct Settings lazily so
    # tests that monkeypatch env vars before entering the TestClient
    # context still work.

    @asynccontextmanager
    async def lifespan(app: FastAPI) -> AsyncIterator[None]:
        """Optionally construct V1 Settings if env vars are present."""
        try:
            from supervoice.shared.config import Settings

            app.state.settings = Settings()  # type: ignore[call-arg]
        except Exception:
            # V1 env vars not set — that's fine for orchestrator-only
            # usage. Store a sentinel so tests can check presence.
            app.state.settings = None
        yield

    inner.router.lifespan_context = lifespan
    return inner


app: FastAPI = _build_app()

__all__ = ["app"]
