"""FastAPI app factory for the V2 orchestrator.

The factory wires injected services onto ``app.state`` and mounts the
public REST routers. Real entrypoint/CLI wiring lives in Task 21.
"""

from __future__ import annotations

from fastapi import FastAPI

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.room.engine import RoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.worker_registry.dispatch import WorkerDispatcher

from .api.dependencies import NumberMappingCache


def create_app(
    *,
    auth_config: AuthConfig,
    room_engine: RoomEngine,
    mapping_cache: NumberMappingCache,
    worker_dispatcher: WorkerDispatcher,
    session_registry: SessionRegistry,
) -> FastAPI:
    """Build the orchestrator FastAPI app with all dependencies bound."""
    app = FastAPI(title="supervoice orchestrator", version="2.0.0")
    app.state.auth_config = auth_config
    app.state.room_engine = room_engine
    app.state.mapping_cache = mapping_cache
    app.state.worker_dispatcher = worker_dispatcher
    app.state.session_registry = session_registry
    app.state.idempotency = {}

    # Import here to avoid circular-import surprises at package load time.
    from .api.dispatch import router as dispatch_router
    from .api.sessions import router as sessions_router

    app.include_router(dispatch_router)
    app.include_router(sessions_router)
    return app


__all__ = ["create_app"]
