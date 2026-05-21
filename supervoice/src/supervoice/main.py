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

from collections.abc import AsyncIterator
from contextlib import asynccontextmanager

from fastapi import FastAPI, WebSocket, WebSocketDisconnect
from loguru import logger
from pipecat.transports.smallwebrtc.connection import SmallWebRTCConnection

from supervoice.config import Settings
from supervoice.pipeline.transport import create_webrtc_transport
from supervoice.session.handler import run_echo_call
from supervoice.speech.stt_factory import STTProviderConfig
from supervoice.speech.tts_factory import TTSProviderConfig


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

    try:
        offer = await ws.receive_json()
    except WebSocketDisconnect:
        return

    connection = SmallWebRTCConnection()
    try:
        await connection.initialize(sdp=offer["sdp"], type=offer["type"])
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

    transport, _detector = create_webrtc_transport(connection)

    stt = STTProviderConfig(
        provider="deepgram",
        api_key=settings.deepgram_api_key,
        language="en",
    )
    tts = TTSProviderConfig(
        provider="cartesia",
        api_key=settings.cartesia_api_key,
        voice_id="sonic-english",  # placeholder; voice profile lands in Task 21
    )

    session_id = getattr(connection, "pc_id", None) or "anon"
    try:
        await run_echo_call(
            session_id=session_id, transport=transport, stt=stt, tts=tts
        )
    except WebSocketDisconnect:
        return
    except Exception as e:
        logger.exception(f"call failed session_id={session_id}: {e}")
