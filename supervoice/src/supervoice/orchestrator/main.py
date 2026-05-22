"""FastAPI app factory for the V2 orchestrator.

The factory wires injected services onto ``app.state`` and mounts the
public REST routers plus backward-compatible endpoints (``/health``,
``/call`` WS shim).
"""

from __future__ import annotations

import uuid

from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from loguru import logger
from starlette.middleware.base import (
    BaseHTTPMiddleware,
    RequestResponseEndpoint,
)
from starlette.requests import Request
from starlette.responses import Response

from supervoice.orchestrator.api.auth import AuthConfig
from supervoice.orchestrator.room.engine import RoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry
from supervoice.orchestrator.worker_registry.dispatch import WorkerDispatcher
from supervoice.shared.observability.logging import request_id_var

from .api.dependencies import NumberMappingCache


class RequestIdMiddleware(BaseHTTPMiddleware):
    """Assign a request_id to every request via contextvars.

    If the client sends an ``X-Request-Id`` header it is reused;
    otherwise a new UUID is generated. The value is echoed back
    in the response header and available via ``get_request_id()``.
    """

    async def dispatch(
        self,
        request: Request,
        call_next: RequestResponseEndpoint,
    ) -> Response:
        rid = request.headers.get("x-request-id") or str(
            uuid.uuid4()
        )
        token = request_id_var.set(rid)
        try:
            response = await call_next(request)
            response.headers["x-request-id"] = rid
            return response
        finally:
            request_id_var.reset(token)


def create_app(
    *,
    auth_config: AuthConfig,
    room_engine: RoomEngine,
    mapping_cache: NumberMappingCache,
    worker_dispatcher: WorkerDispatcher,
    session_registry: SessionRegistry,
    dev_mode: bool = False,
) -> FastAPI:
    """Build the orchestrator FastAPI app with all dependencies bound.

    Parameters
    ----------
    dev_mode:
        When ``True``, mounts the ``/v1/dev/*`` router (audio injection
        and other dev-only endpoints). Disabled by default so production
        deployments never expose dev routes.
    """
    app = FastAPI(title="supervoice orchestrator", version="2.0.0")
    app.add_middleware(RequestIdMiddleware)
    app.state.auth_config = auth_config
    app.state.room_engine = room_engine
    app.state.mapping_cache = mapping_cache
    app.state.worker_dispatcher = worker_dispatcher
    app.state.session_registry = session_registry
    app.state.idempotency = {}
    app.state.dev_mode = dev_mode

    # Import here to avoid circular-import surprises at package load time.
    from .api.dispatch import router as dispatch_router
    from .api.sessions import router as sessions_router

    app.include_router(dispatch_router)
    app.include_router(sessions_router)

    if dev_mode:
        from .api.dev import router as dev_router

        app.include_router(dev_router)

    # -- Backward-compatible endpoints -------------------------------------

    @app.get("/health")
    async def health() -> dict[str, str]:
        """Liveness probe used by container/orchestrator health checks."""
        return {"status": "ok"}

    @app.websocket("/call")
    async def call_shim(ws: WebSocket, profile: str = "en-female") -> None:
        """V1-compatible WebSocket endpoint.

        Accepts a WS connection with an optional ``?profile=`` query
        param, receives the SDP offer JSON, dispatches a session
        through the same code path as ``POST /v1/dispatch``, and
        returns the SDP answer. The WS stays open until the client
        disconnects (at which point the session is ended).

        This is a *shim* — full WebRTC media is not handled here.
        Actual media will flow through LiveKit once the transport
        migration is complete.
        """
        await ws.accept()

        # Read SDP offer from client.
        try:
            offer = await ws.receive_json()
            sdp = offer["sdp"]
            sdp_type = offer["type"]
        except (KeyError, ValueError) as exc:
            logger.warning("call shim: malformed SDP offer: %s", exc)
            await ws.close(code=1003)
            return
        except WebSocketDisconnect:
            return

        # Dispatch internally — reuse the service layer directly
        # instead of making an HTTP round-trip to ourselves.
        from supervoice.orchestrator.api.dispatch import (
            _room_join_from_handle,
            _synthesize_sdp_answer,
        )
        from supervoice.orchestrator.room.engine import RoomOpts
        from supervoice.orchestrator.session.state import Session

        session_id = f"s-{uuid.uuid4().hex[:16]}"
        metadata = {
            "profile": profile,
            "direction": "inbound",
            "transport": "webrtc-ws-shim",
        }
        session = Session(
            session_id=session_id,
            tenant_id="ws-shim",
            metadata=metadata,
        )
        registry: SessionRegistry = app.state.session_registry
        await registry.register(session)

        engine: RoomEngine = app.state.room_engine  # type: ignore[assignment]
        try:
            room_handle = await engine.create_room(
                RoomOpts(session_id=session_id, metadata=metadata)
            )
        except Exception as exc:
            logger.exception("call shim: room creation failed: %s", exc)
            await ws.close(code=1011)
            return

        session.room_handle = room_handle

        # Attach a webrtc participant with the SDP offer.
        try:
            participant = await engine.add_media_participant(
                room_handle,
                "webrtc",
                {"sdp_offer": sdp, "type": sdp_type},
            )
        except Exception as exc:
            logger.exception("call shim: add_media_participant failed: %s", exc)
            await ws.close(code=1011)
            return

        sdp_answer = _synthesize_sdp_answer(participant)
        room_join = _room_join_from_handle(room_handle)

        # Send the SDP answer back to the client.
        await ws.send_json(
            {
                "sdp": sdp_answer,
                "type": "answer",
                "session_id": session_id,
                "room": room_join.model_dump(),
            }
        )

        session.transition("ringing")

        # Hold the WS open until the client disconnects.
        try:
            while True:
                await ws.receive_text()
        except WebSocketDisconnect:
            pass
        finally:
            # Clean up the session on disconnect.
            if session.state not in {
                "ended",
                "rejected",
                "timed_out",
                "failed",
            }:
                try:
                    session.transition("ended")
                except ValueError:
                    pass
            try:
                await engine.destroy_room(room_handle, graceful=True)
            except Exception:
                pass
            logger.info(
                "call shim: session %s ended on WS disconnect",
                session_id,
            )

    return app


def create_single_process_app(
    *,
    auth_config: AuthConfig | None = None,
    mapping_cache: NumberMappingCache | None = None,
    dev_mode: bool = True,
) -> FastAPI:
    """Create an app with both orchestrator and worker in one process.

    Wires a ``WorkerRegistry``, ``WorkerDispatcher``, and
    ``WorkerDispatchServer`` (orchestrator side) together with a
    ``JobRunner`` + ``WorkerRegistration`` (worker side) using
    in-memory queues -- no external WSS needed.

    This is the ``--single-process`` convenience for local dev. The
    returned app has ``app.state.worker_registration`` and
    ``app.state.job_runner`` for lifecycle management.

    Parameters
    ----------
    auth_config:
        Auth configuration. Defaults to a permissive dev config with
        a ``dev-mode`` tenant secret.
    mapping_cache:
        Number mapping cache. Defaults to ``None`` (a stub is created).
    dev_mode:
        Mount dev endpoints. Defaults to ``True`` in single-process.
    """
    import asyncio
    from typing import Any

    from supervoice.orchestrator.room.in_process_engine import (
        InProcessRoomEngine,
    )
    from supervoice.orchestrator.worker_registry import (
        WorkerDispatchServer,
        WorkerRegistry,
    )
    from supervoice.shared.dispatch_protocol import WorkerCapabilities
    from supervoice.shared.voice_profile.catalog import VoiceProfileCatalog
    from supervoice.worker.job_runner import JobRunner
    from supervoice.worker.registration import WorkerLink, WorkerRegistration

    if auth_config is None:
        from supervoice.orchestrator.api.auth import TenantSecret

        auth_config = AuthConfig(
            secrets=[
                TenantSecret(
                    tenant_id="dev-mode",
                    secret="dev-secret",
                    admin=True,
                )
            ]
        )

    if mapping_cache is None:
        from supervoice.orchestrator.api.dependencies import AgentConfig

        class _StubMappingCache:
            """Minimal in-memory mapping cache for dev."""

            def __init__(self) -> None:
                self._data: dict[tuple[str, str], AgentConfig] = {}

            async def get(
                self, *, tenant_id: str, to_number: str
            ) -> AgentConfig | None:
                return self._data.get((tenant_id, to_number))

            async def upsert(
                self,
                *,
                tenant_id: str,
                to_number: str,
                config: AgentConfig,
            ) -> None:
                self._data[(tenant_id, to_number)] = config

        mapping_cache = _StubMappingCache()  # type: ignore[assignment]

    # -- Orchestrator services ------------------------------------------------
    registry = WorkerRegistry(heartbeat_timeout_s=60.0)
    dispatcher = WorkerDispatcher(registry, dispatch_timeout_s=5.0)
    shared_secret = "single-process-secret"  # noqa: S105
    server = WorkerDispatchServer(
        registry=registry,
        dispatcher=dispatcher,
        shared_secret=shared_secret,
        heartbeat_interval_s=30,
    )

    room_engine = InProcessRoomEngine()
    session_registry = SessionRegistry()

    # -- In-memory queues (worker <-> orchestrator) ---------------------------
    w2o: asyncio.Queue[dict[str, Any]] = asyncio.Queue()
    o2w: asyncio.Queue[dict[str, Any]] = asyncio.Queue()

    async def server_send(frame: dict[str, Any]) -> None:
        await o2w.put(frame)

    async def server_recv() -> dict[str, Any]:
        return await w2o.get()

    # -- Worker services ------------------------------------------------------
    class _QueueLink:
        """In-memory WorkerLink for single-process mode."""

        def __init__(
            self,
            outbound: asyncio.Queue[dict[str, Any]],
            inbound: asyncio.Queue[dict[str, Any]],
        ) -> None:
            self._outbound = outbound
            self._inbound = inbound
            self.closed = False

        async def send(self, frame: dict[str, Any]) -> None:
            await self._outbound.put(frame)

        async def recv(self) -> dict[str, Any]:
            return await self._inbound.get()

        async def close(self) -> None:
            self.closed = True

    async def upstream_send(frame: dict[str, Any]) -> None:
        await w2o.put(frame)

    job_runner = JobRunner(
        max_concurrent=2,
        api_keys={},
        catalog=VoiceProfileCatalog.load_default(),
        upstream_send=upstream_send,
    )

    link = _QueueLink(outbound=w2o, inbound=o2w)

    async def link_factory(
        _url: str, _secret: str
    ) -> WorkerLink:
        return link  # type: ignore[return-value]

    worker_registration = WorkerRegistration(
        orchestrator_url="ws://localhost",
        shared_secret=shared_secret,
        worker_id="w-single-process",
        pool="default",
        capabilities=WorkerCapabilities(
            voice_profiles=["en-female"],
            max_concurrent=2,
        ),
        dispatch_handler=job_runner.accept,
        active_jobs_counter=job_runner.active_count,
        link_factory=link_factory,
        reconnect_delay_s=0.0,
    )

    # -- Build the app --------------------------------------------------------
    app = create_app(
        auth_config=auth_config,
        room_engine=room_engine,
        mapping_cache=mapping_cache,
        worker_dispatcher=dispatcher,
        session_registry=session_registry,
        dev_mode=dev_mode,
    )

    # Expose worker-side handles on app.state for lifecycle management.
    app.state.worker_registry = registry
    app.state.worker_dispatch_server = server
    app.state.worker_registration = worker_registration
    app.state.job_runner = job_runner
    app.state.single_process = True

    # Lifespan tasks: start the server accept loop + worker registration
    # as background tasks when the app starts up.
    _background_tasks: list[asyncio.Task[None]] = []

    @app.on_event("startup")
    async def _start_single_process() -> None:
        server_task = asyncio.create_task(
            server.accept(
                server_send,
                server_recv,
                presented_secret=shared_secret,
            )
        )
        worker_task = asyncio.create_task(worker_registration.run())
        _background_tasks.extend([server_task, worker_task])
        logger.info("single-process mode: worker + server started")

    @app.on_event("shutdown")
    async def _stop_single_process() -> None:
        await worker_registration.close()
        for task in _background_tasks:
            task.cancel()
        logger.info("single-process mode: worker + server stopped")

    return app


__all__ = ["create_app", "create_single_process_app"]
