"""Dev-only endpoints mounted when ``--dev-mode`` is active.

``POST /v1/dev/inject-audio`` injects a WAV file as synthetic user
audio into an existing session. V1 scope: creates a synthetic
participant in the session's room and validates plumbing. Actual
audio-frame streaming through STT is deferred to Phase 6+.
"""

from __future__ import annotations

from fastapi import APIRouter, File, Form, HTTPException, Request, UploadFile

from supervoice.orchestrator.room.engine import RoomEngine
from supervoice.orchestrator.session.registry import SessionRegistry

router = APIRouter(prefix="/v1/dev", tags=["dev"])


@router.post("/inject-audio")
async def inject_audio(
    request: Request,
    session_id: str = Form(...),
    file: UploadFile = File(...),
    play_as: str = Form("user_speaking"),
    loop: bool = Form(False),
) -> dict:
    """Inject a WAV file as synthetic user audio into a session.

    Available only in ``--dev-mode``. The audio is fed to the session's
    room engine as if a real participant were speaking.

    V1 scope: reads the WAV into memory, creates a synthetic participant
    in the session's room via the engine. Actual audio streaming into
    the STT pipeline requires the in_process_engine to support audio
    frame fan-out -- for V1, this endpoint verifies the plumbing works
    (creates the participant, validates the session exists) but does
    NOT actually feed audio frames through STT. Real audio injection
    is Phase 6+.
    """
    registry: SessionRegistry = request.app.state.session_registry
    session = await registry.get(session_id, tenant_id="dev-mode")
    if session is None:
        raise HTTPException(404, detail="session not found")

    engine: RoomEngine = request.app.state.room_engine  # type: ignore[assignment]
    if session.room_handle is None:
        raise HTTPException(409, detail="session has no room")

    audio_data = await file.read()

    try:
        participant = await engine.add_media_participant(
            session.room_handle,
            "webrtc",
            {
                "synthetic": True,
                "audio_data": audio_data,
                "play_as": play_as,
                "loop": loop,
            },
        )
    except RuntimeError as e:
        raise HTTPException(409, detail=str(e)) from e

    return {
        "status": "injected",
        "session_id": session_id,
        "participant_id": participant.participant_id,
        "audio_size_bytes": len(audio_data),
        "play_as": play_as,
        "loop": loop,
    }
