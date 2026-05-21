"""FastAPI application shell for the supervoice service.

Exposes:

* ``GET /health`` — liveness probe.
* ``WS  /call`` — WebRTC signaling + audio path. The client sends an SDP
  offer as a JSON message ``{"sdp": ..., "type": "offer"}``; the server
  replies with the SDP answer and then runs the echo pipeline for the
  lifetime of the WebRTC peer connection.

``Settings`` is intentionally constructed inside the lifespan context
manager (not at module import) so tests and tooling can import ``app``
without the runtime env vars being present.
"""

from __future__ import annotations

import asyncio
import json
from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from loguru import logger
from pipecat.transports.smallwebrtc.connection import SmallWebRTCConnection

from supervoice.config import Settings
from supervoice.pipeline.transport import create_webrtc_transport
from supervoice.session.handler import run_call_with_profile


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
async def call_endpoint(ws: WebSocket, profile: str = "en-female") -> None:
    """WebRTC signaling endpoint that hands off to the echo call handler.

    Protocol (v1, minimal):
        1. Client connects to ``ws://host/call``.
        2. Client sends JSON ``{"sdp": "...", "type": "offer"}``.
        3. Server initializes ``SmallWebRTCConnection``, replies with
           ``{"sdp": "...", "type": "answer", "pc_id": "..."}``.
        4. Server runs the echo pipeline; the WS stays open until the
           peer connection or socket closes.
    """
    settings: Settings = app.state.settings
    await ws.accept()

    connection = SmallWebRTCConnection()

    # v1 only does single-shot SDP offer/answer over the WS. Trickle ICE
    # candidate exchange after the answer isn't implemented; clients that
    # require it will see incomplete connectivity (follow-up).
    try:
        offer = await asyncio.wait_for(
            ws.receive_json(), timeout=settings.webrtc_handshake_timeout_s
        )
        sdp = offer["sdp"]
        sdp_type = offer["type"]
    except asyncio.TimeoutError:
        logger.warning("webrtc handshake timeout")
        await ws.close(code=1008)
        return
    except (json.JSONDecodeError, KeyError, ValueError) as e:
        logger.warning(f"malformed sdp offer: {e}")
        await ws.close(code=1003)
        return
    except WebSocketDisconnect:
        return

    try:
        await connection.initialize(sdp=sdp, type=sdp_type)
    except Exception as e:
        logger.exception(f"SDP initialize failed: {e}")
        await ws.close()
        return

    answer = connection.get_answer()
    if answer is None:
        logger.error("SmallWebRTC connection produced no SDP answer")
        await ws.close()
        return
    await ws.send_json(answer)

    # detector wires VAD/EOU into the transport — the pipeline doesn't need
    # a direct handle yet.
    transport, _detector = create_webrtc_transport(connection)

    api_keys = {
        "deepgram": settings.deepgram_api_key,
        "cartesia": settings.cartesia_api_key,
    }
    if settings.elevenlabs_api_key is not None:
        api_keys["elevenlabs"] = settings.elevenlabs_api_key

    session_id = getattr(connection, "pc_id", None) or "anon"
    try:
        await run_call_with_profile(
            session_id=session_id,
            transport=transport,
            profile_id=profile,
            api_keys=api_keys,
            agent_bridge_url=settings.agent_bridge_url,
        )
    except asyncio.CancelledError:
        raise
    except WebSocketDisconnect:
        return
    except Exception:
        logger.exception(f"call failed session_id={session_id}")
