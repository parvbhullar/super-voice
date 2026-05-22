"""FastAPI app factory for the V2 orchestrator.

The factory wires injected services onto ``app.state`` and mounts the
public REST routers plus backward-compatible endpoints (``/health``,
``/call`` WS shim).
"""

from __future__ import annotations

from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from loguru import logger

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
        import uuid

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


__all__ = ["create_app"]
